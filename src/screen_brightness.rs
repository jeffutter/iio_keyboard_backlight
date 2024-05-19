use anyhow::Result;
use log::{debug, info};
use logind_zbus::session::SessionProxyBlocking;

use crate::read_value;

pub(crate) struct ScreenBrightness<'a, 'b> {
    proxy: &'a SessionProxyBlocking<'b>,
    subsystem: &'a str,
    name: &'a str,
    max_brightness: u32,
}

impl<'a, 'b> ScreenBrightness<'a, 'b> {
    pub(crate) fn new(
        proxy: &'a SessionProxyBlocking<'b>,
        subsystem: &'a str,
        name: &'a str,
    ) -> Result<Self> {
        let max_brightness =
            read_value(&format!("/sys/class/{}/{}/max_brightness", subsystem, name))?;

        Ok(Self {
            proxy,
            subsystem,
            name,
            max_brightness,
        })
    }

    fn read(&self) -> Result<u32> {
        read_value(&format!(
            "/sys/class/{}/{}/brightness",
            self.subsystem, self.name
        ))
    }

    fn pct_to_brightness(&self, pct: u32) -> u32 {
        (pct * (self.max_brightness)) / 100
    }

    pub(crate) fn adjust(&self, new_val: u32) -> Result<()> {
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
