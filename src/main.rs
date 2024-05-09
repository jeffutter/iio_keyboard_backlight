use std::{
    error::Error,
    fs,
    ops::{Div, Mul},
    thread::sleep,
    time::Duration,
};

use industrial_io::Context;
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
    let connection = Connection::system()?;
    let proxy = SessionProxyBlocking::builder(&connection)
        .path("/org/freedesktop/login1/session/auto")?
        .build()?;

    let ctx = Context::new()?;
    let max_brightness = read_value("max_brightness")?;
    let max_brightness_f64 = max_brightness as f64;
    let max_reading = 10000u32;
    let max_reading_f64 = max_reading as f64;

    let dev = ctx.find_device("als").expect("Couldn't find als device");
    let chan = dev.get_channel(0)?;
    let initial = chan.attr_read_int("raw")?;
    let mut wma = WMA::new(10, &(initial as f64))?;

    loop {
        let val = chan.attr_read_int("raw")? as f64;
        let max_val = val.min(max_reading_f64);
        let new_val = wma.next(&max_val);

        let new_target = (max_brightness
            - (max_brightness_f64.mul(new_val).div(max_reading_f64).floor() as u32))
            .min(max_brightness);

        let cur_brightness = read_value("brightness")?;

        if cur_brightness != new_target {
            println!(
                "Adjusting KBD Backlight: val:{:?} old:{:?} new:{:?}",
                new_val, cur_brightness, new_target
            );
            proxy.set_brightness("leds", "asus::kbd_backlight", new_target)?;
        }

        sleep(Duration::from_secs(5));
    }
}
