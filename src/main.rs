use std::{
    error::Error,
    fs,
    ops::{Div, Mul},
    thread::sleep,
    time::Duration,
};

use env_logger::Env;
use industrial_io::Context;
use log::{debug, info};
use logind_zbus::session::SessionProxyBlocking;
use yata::{core::Method, methods::WMA};
use zbus::blocking::Connection;

fn read_value(name: &str) -> Result<u32, Box<dyn Error>> {
    let filename = format!("/sys/class/leds/asus::kbd_backlight/{}", name);
    let val = fs::read_to_string(filename)?;
    let res = val.trim().parse()?;
    Ok(res)
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_env(Env::default().default_filter_or("warn")).init();

    let connection = Connection::system()?;
    let proxy = SessionProxyBlocking::builder(&connection)
        .path("/org/freedesktop/login1/session/auto")?
        .build()?;

    let ctx = Context::new()?;
    let max_brightness = read_value("max_brightness")?;
    debug!("Max Brightness: {}", max_brightness);
    let max_brightness_f64 = max_brightness as f64;
    let max_reading = 10000u32;
    let max_reading_f64 = max_reading as f64;

    let dev = ctx.find_device("als").expect("Couldn't find als device");
    let chan = dev.get_channel(0)?;
    let initial = chan.attr_read_int("raw")?.min(max_reading as i64);
    let mut wma = WMA::new(10, &(initial as f64))?;

    loop {
        let val = chan.attr_read_int("raw")? as f64;
        debug!("Val: {}", val);
        let max_val = val.min(max_reading_f64);
        debug!("Max Val: {}", max_val);
        let new_val = wma.next(&max_val);
        debug!("New Val: {}", new_val);

        let target = max_brightness_f64.mul(new_val).div(max_reading_f64);
        debug!("Target Val: {}", target);

        let new_target = (max_brightness - (target.floor() as u32)).min(max_brightness);
        debug!("New Target: {}", new_target);

        let cur_brightness = read_value("brightness")?;
        debug!("Current Brightness: {}", cur_brightness);

        if cur_brightness != new_target {
            info!(
                "Adjusting KBD Backlight: val:{:?} old:{:?} new:{:?}",
                new_val, cur_brightness, new_target
            );
            proxy.set_brightness("leds", "asus::kbd_backlight", new_target)?;
        }

        sleep(Duration::from_secs(5));
    }
}
