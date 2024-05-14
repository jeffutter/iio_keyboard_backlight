use std::{error::Error, fs, thread::sleep, time::Duration};

use env_logger::Env;
use industrial_io::{Channel, Context};
use log::{debug, info, trace};
use logind_zbus::session::SessionProxyBlocking;
use yata::{core::Method, methods::WMA};
use zbus::blocking::Connection;

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
            v if v < 1 => 3,
            v if v < 2 => 2,
            v if v < 3 => 1,
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
            v if v < 2 => 15,
            v if v < 5 => 30,
            v if v < 10 => 50,
            v if v < 25 => 80,
            _ => 90,
        };

        let new_level = self.pct_to_brightness(new_pct);

        let cur_brightness = self.read()?;

        debug!(
            "Backlight: nv:{:?}, nl:{:?}, cb:{:?}",
            new_val, new_level, cur_brightness
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
    wma: WMA,
}

impl AmbientBrightness {
    fn new() -> Result<Self, Box<dyn Error>> {
        let ctx = Context::new()?;

        let max = 2500000u32;
        let dev = ctx.find_device("als").expect("Couldn't find als device");
        let chan = dev.get_channel(0)?;

        let initial = chan.attr_read_int("raw")?.min(max as i64);
        let wma = WMA::new(10, &(initial as f64))?;

        Ok(Self { chan, max, wma })
    }

    fn update(&mut self) -> Result<u32, Box<dyn Error>> {
        let val = self.chan.attr_read_int("raw")? as f64;
        trace!("Val: {}", val);
        let max_val = val.min(self.max as f64);
        trace!("Max Val: {}", max_val);
        let new_val = self.wma.next(&max_val);
        trace!("New Val: {}", new_val);
        let new_pct = (new_val * 100f64) / self.max as f64;
        trace!("New PCT: {}", new_pct);
        Ok(new_pct.round() as u32)
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_env(Env::default().default_filter_or("warn")).init();

    let connection = Connection::system()?;
    let proxy = SessionProxyBlocking::builder(&connection)
        .path("/org/freedesktop/login1/session/auto")?
        .build()?;

    let mut ambient_brightness = AmbientBrightness::new()?;
    let kbd_brightness = KBDBrightness::new(&proxy, "leds", "asus::kbd_backlight");
    let screen_brightness = ScreenBrightness::new(&proxy, "backlight", "intel_backlight")?;

    loop {
        let new_val = ambient_brightness.update()?;
        trace!("New Val POST: {}", new_val);
        kbd_brightness.adjust(new_val)?;
        screen_brightness.adjust(new_val)?;

        sleep(Duration::from_secs(5));
    }
}
