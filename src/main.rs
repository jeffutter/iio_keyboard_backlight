use std::{
    env,
    error::Error,
    fs,
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    thread,
    time::Duration,
};

use clap::Parser;
use crossbeam::{
    channel::{after, bounded, tick},
    select,
};
use env_logger::Env;
use industrial_io::{Channel, Context};
use log::{debug, info, trace};
use logind_zbus::session::SessionProxyBlocking;
use yata::{core::Method, methods::WMA};
use zbus::{
    blocking::Connection,
    zvariant::{Endian, ReadBytes, WriteBytes},
};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[arg(short, default_value_t = false)]
    server: bool,

    #[arg(short, conflicts_with = "server", default_value_t = false)]
    dim: bool,
}

fn read_value(path: &str) -> Result<u32, Box<dyn Error>> {
    let val = fs::read_to_string(path)?;
    let res = val.trim().parse()?;
    Ok(res)
}

struct KBDBrightness<'a, 'b> {
    proxy: &'a SessionProxyBlocking<'b>,
    subsystem: &'a str,
    name: &'a str,
}

impl<'a, 'b> KBDBrightness<'a, 'b> {
    fn new(proxy: &'a SessionProxyBlocking<'b>, subsystem: &'a str, name: &'a str) -> Self {
        Self {
            proxy,
            subsystem,
            name,
        }
    }

    fn read(&self) -> Result<u32, Box<dyn Error>> {
        read_value(&format!(
            "/sys/class/{}/{}/brightness",
            self.subsystem, self.name
        ))
    }

    fn adjust(&self, new_val: u32) -> Result<(), Box<dyn Error>> {
        let new_level = match new_val {
            v if v < 50 => 3,
            v if v < 60 => 2,
            v if v < 80 => 1,
            _ => 0,
        };

        let cur_brightness = self.read()?;

        debug!(
            "KBD: nv:{:?}, nl:{:?}, cb:{:?}",
            new_val, new_level, cur_brightness
        );
        if cur_brightness != new_level {
            info!(
                "Adjusting KBD Backlight: val:{:?} old:{:?} new:{:?}",
                new_val, cur_brightness, new_level
            );
            self.proxy
                .set_brightness(self.subsystem, self.name, new_level)?;
        }

        Ok(())
    }
}

struct ScreenBrightness<'a, 'b> {
    proxy: &'a SessionProxyBlocking<'b>,
    subsystem: &'a str,
    name: &'a str,
    max_brightness: u32,
}

impl<'a, 'b> ScreenBrightness<'a, 'b> {
    fn new(
        proxy: &'a SessionProxyBlocking<'b>,
        subsystem: &'a str,
        name: &'a str,
    ) -> Result<Self, Box<dyn Error>> {
        let max_brightness =
            read_value(&format!("/sys/class/{}/{}/max_brightness", subsystem, name))?;

        Ok(Self {
            proxy,
            subsystem,
            name,
            max_brightness,
        })
    }

    fn read(&self) -> Result<u32, Box<dyn Error>> {
        read_value(&format!(
            "/sys/class/{}/{}/brightness",
            self.subsystem, self.name
        ))
    }

    fn pct_to_brightness(&self, pct: u32) -> u32 {
        (pct * (self.max_brightness)) / 100
    }

    fn adjust(&self, new_val: u32) -> Result<(), Box<dyn Error>> {
        let new_pct = match new_val {
            v if v < 1 => 5,
            v if v < 10 => 10,
            v if v < 20 => 15,
            v if v < 30 => 20,
            v if v < 40 => 25,
            v if v < 50 => 30,
            v if v < 60 => 35,
            v if v < 70 => 40,
            v if v < 80 => 45,
            _ => 50,
        };

        let new_level = self.pct_to_brightness(new_pct);

        let cur_brightness = self.read()?;

        debug!(
            "Backlight: nv:{:?}, np:{:?}, nl:{:?}, cb:{:?}",
            new_val, new_pct, new_level, cur_brightness
        );
        if cur_brightness != new_level {
            info!(
                "Adjusting Screen Backlight: val:{:?} old:{:?} new:{:?}->{:?}",
                new_val, cur_brightness, new_pct, new_level
            );
            self.proxy
                .set_brightness(self.subsystem, self.name, new_level)?;
        }

        Ok(())
    }
}

struct AmbientBrightness {
    chan: Channel,
    max: u32,
    wma: Option<WMA>,
    dim: bool,
}

impl AmbientBrightness {
    fn new() -> Result<Self, Box<dyn Error>> {
        let ctx = Context::new()?;

        let max = (2500000u32).ilog10();
        let dev = ctx.find_device("als").expect("Couldn't find als device");
        let chan = dev.get_channel(0)?;

        Ok(Self {
            chan,
            max,
            wma: None,
            dim: false,
        })
    }

    fn init(mut self) -> Result<Self, Box<dyn Error>> {
        let initial = self.read()?;
        let wma = WMA::new(10, &initial)?;
        self.wma = Some(wma);
        Ok(self)
    }

    fn read(&self) -> Result<f64, Box<dyn Error>> {
        Ok((self.chan.attr_read_int("raw")? as f64).log10())
    }

    fn update(&mut self) -> Result<u32, Box<dyn Error>> {
        let val = self.read()?;
        trace!("Val: {}", val);
        let max_val = val.min(self.max as f64);
        trace!("Max Val: {}", max_val);
        let new_val = self
            .wma
            .as_mut()
            .expect("AmbientBrightness not Initialized")
            .next(&max_val);
        trace!("New Val: {}", new_val);
        let new_pct = (new_val * 100f64) / self.max as f64;
        trace!("New PCT: {}", new_pct);

        let dimmed = if self.dim { new_pct / 4f64 } else { new_pct };
        trace!("Dimmed: {}", dimmed);

        debug!(
            "Ambient - val:{:.4}, max_val:{:.4}, new_val:{:.4}, new_pct:{:.4}, dimmed:{:.4}",
            val, max_val, new_val, new_pct, dimmed
        );
        Ok(dimmed.round() as u32)
    }

    fn dim(&mut self) {
        self.dim = true;
    }

    fn undim(&mut self) {
        self.dim = false;
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_env(Env::default().default_filter_or("warn")).init();
    let args = Args::parse();

    if args.server {
        let connection = Connection::system()?;
        let proxy = SessionProxyBlocking::builder(&connection)
            .path("/org/freedesktop/login1/session/auto")?
            .build()?;

        let mut ambient_brightness = AmbientBrightness::new()?.init()?;
        let kbd_brightness = KBDBrightness::new(&proxy, "leds", "asus::kbd_backlight");
        let screen_brightness = ScreenBrightness::new(&proxy, "backlight", "intel_backlight")?;
        let (command_sender, command_receiver) = bounded(1);

        thread::spawn(move || {
            let res = || -> Result<(), Box<dyn Error>> {
                let socket_path = Path::new(&env::temp_dir()).join("ambient_brightness.sock");
                fs::remove_file(socket_path.clone())?;
                let listener = UnixListener::bind(socket_path)?;

                loop {
                    let (mut socket, _addr) = listener.accept()?;
                    let socket_read = socket.read_u8(Endian::native())?;
                    debug!("Got Message: {}", socket_read);

                    match socket_read {
                        0 => command_sender.send(0)?,
                        1 => command_sender.send(1)?,
                        _ => (),
                    }
                }
            }();

            res.unwrap();
        });

        let ticker = tick(Duration::from_secs(5));
        let first = after(Duration::from_millis(1));

        let update = |ambient_brightness: &mut AmbientBrightness| -> Result<(), Box<dyn Error>> {
            let new_val = ambient_brightness.update()?;
            trace!("New Val POST: {}", new_val);
            kbd_brightness.adjust(new_val)?;
            screen_brightness.adjust(new_val)?;
            Ok(())
        };

        loop {
            select! {
                recv(command_receiver) -> msg => match msg? {
                    0 => {
                        ambient_brightness.dim();
                        update(&mut ambient_brightness)?
                    },

                    1 => {
                        ambient_brightness.undim();
                        update(&mut ambient_brightness)?
                    },
                    _ => ()
                },
                recv(ticker) -> _  => {
                    update(&mut ambient_brightness)?
                },
                recv(first) -> _ => {
                    update(&mut ambient_brightness)?
                },
            }
        }
    } else {
        let socket_path = Path::new(&env::temp_dir()).join("ambient_brightness.sock");
        let mut client = UnixStream::connect(socket_path)?;
        if args.dim {
            client.write_u8(Endian::native(), 0)?;
        } else {
            client.write_u8(Endian::native(), 1)?;
        }
        client.flush()?;
        info!("Done");
    }

    Ok(())
}
