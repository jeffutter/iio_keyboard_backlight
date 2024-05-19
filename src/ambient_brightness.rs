use anyhow::Result;
use industrial_io::{Channel, Context};
use log::{debug, trace};
use yata::{core::Method, methods::WMA};

pub(crate) struct AmbientBrightness {
    chan: Channel,
    max: u32,
    wma: Option<WMA>,
    dim: bool,
}

impl AmbientBrightness {
    pub(crate) fn new() -> Result<Self> {
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

    pub(crate) fn init(mut self) -> Result<Self> {
        let initial = self.read()?;
        let wma = WMA::new(10, &initial)?;
        self.wma = Some(wma);
        Ok(self)
    }

    fn read(&self) -> Result<f64> {
        Ok((self.chan.attr_read_int("raw")? as f64).log10())
    }

    pub(crate) fn update(&mut self) -> Result<u32> {
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

    pub(crate) fn dim(&mut self) {
        self.dim = true;
    }

    pub(crate) fn undim(&mut self) {
        self.dim = false;
    }
}
