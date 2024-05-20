mod ambient_brightness;
mod control_client;
mod control_server;
mod kbd_brightness;
mod screen_brightness;

use std::{
    fs,
    sync::atomic::{self, AtomicBool},
    time::Duration,
};

use ambient_brightness::AmbientBrightness;
use anyhow::{anyhow, Result};
use clap::Parser;
use crossbeam::{
    channel::{bounded, tick, Receiver},
    select,
};
use env_logger::Env;
use kbd_brightness::KBDBrightness;
use log::{info, trace};
use logind_zbus::session::SessionProxyBlocking;
use ouroboros::self_referencing;
use screen_brightness::ScreenBrightness;
use zbus::blocking::Connection;

use crate::{control_client::ControlClient, control_server::ControlServer};

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

#[self_referencing]
struct AmbientBrightnessController<'a> {
    ambient_brightness: AmbientBrightness,
    proxy: SessionProxyBlocking<'a>,
    #[borrows(proxy)]
    #[not_covariant]
    kbd_brightness: KBDBrightness<'this>,
    #[borrows(proxy)]
    #[not_covariant]
    screen_brightness: ScreenBrightness<'this>,
    close_receiver: Receiver<()>,
    command_receiver: Receiver<u8>,
}

impl<'a> AmbientBrightnessController<'a> {
    fn create(close_receiver: Receiver<()>, command_receiver: Receiver<u8>) -> Result<Self> {
        let connection = Connection::system()?;
        let proxy = SessionProxyBlocking::builder(&connection)
            .path("/org/freedesktop/login1/session/auto")?
            .build()?;

        let ambient_brightness = AmbientBrightness::new()?.init()?;

        Self::try_new(
            ambient_brightness,
            proxy,
            |proxy: &SessionProxyBlocking| {
                Ok(KBDBrightness::new(proxy, "leds", "asus::kbd_backlight"))
            },
            |proxy: &SessionProxyBlocking| {
                ScreenBrightness::new(proxy, "backlight", "intel_backlight")
            },
            close_receiver,
            command_receiver,
        )
    }

    fn update(&mut self) -> Result<()> {
        let new_val = self.with_ambient_brightness_mut(|x| x.update())?;
        trace!("New Val POST: {}", new_val);
        self.with_kbd_brightness(|x| x.adjust(new_val))?;
        self.with_screen_brightness(|x| x.adjust(new_val))?;
        Ok(())
    }

    fn run(mut self) -> Result<()> {
        let ticker = tick(Duration::from_secs(5));
        self.update()?;

        loop {
            select! {
                recv(self.borrow_close_receiver()) -> _ => {
                    break
                },
                recv(self.borrow_command_receiver()) -> msg => match msg? {
                    0 => {
                        self.with_ambient_brightness_mut(|x| x.dim());
                        self.update()?
                    },

                    1 => {
                        self.with_ambient_brightness_mut(|x| x.undim());
                        self.update()?
                    },
                    _ => ()
                },
                recv(ticker) -> _  => {
                        self.update()?
                },
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("warn")).init();
    let exit_bool = AtomicBool::new(false);
    let exit_bool1 = AtomicBool::new(false);
    let (close_sender, close_receiver) = bounded(1);
    ctrlc::set_handler(move || {
        exit_bool.store(true, atomic::Ordering::Relaxed);
        close_sender
            .send(())
            .expect("Could not send signal on channel.")
    })
    .expect("Error setting Ctrl-C handler");
    let args = Args::parse();

    if args.server {
        let (control_server, command_receiver) = ControlServer::new()?;
        let ambient_brightness_controller =
            AmbientBrightnessController::create(close_receiver, command_receiver)?;
        let join_handle = control_server.run(exit_bool1);

        ambient_brightness_controller.run()?;

        info!("Waiting for Server Thread to stop.");
        join_handle
            .join()
            .map_err(|e| anyhow!("Error waiting for Server Thread: {:?}", e))??;
    } else {
        let mut client = ControlClient::new()?;

        if args.dim {
            client.dim()?;
        } else {
            client.undim()?;
        }
        info!("Done");
    }

    Ok(())
}
