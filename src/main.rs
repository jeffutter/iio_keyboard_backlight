mod ambient_brightness;
mod kbd_brightness;
mod screen_brightness;

use std::{
    env, fs,
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    thread,
    time::Duration,
};

use ambient_brightness::AmbientBrightness;
use anyhow::Result;
use clap::Parser;
use crossbeam::{
    channel::{after, bounded, tick},
    select,
};
use env_logger::Env;
use kbd_brightness::KBDBrightness;
use log::{debug, info, trace};
use logind_zbus::session::SessionProxyBlocking;
use screen_brightness::ScreenBrightness;
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

fn read_value(path: &str) -> Result<u32> {
    let val = fs::read_to_string(path)?;
    let res = val.trim().parse()?;
    Ok(res)
}

fn main() -> Result<()> {
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
            let res = || -> Result<()> {
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

        let update = |ambient_brightness: &mut AmbientBrightness| -> Result<()> {
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
