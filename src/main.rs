mod ambient_brightness;
mod control_client;
mod control_server;
mod kbd_brightness;
mod screen_brightness;

use std::{
    fs,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
    time::Duration,
};

use ambient_brightness::AmbientBrightness;
use anyhow::{anyhow, Context, Result};
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

use crate::{
    control_client::ControlClient,
    control_server::{Command, ControlServer},
};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Server
    #[arg(
        short,
        required_unless_present = "activity",
        required_unless_present = "offset",
        conflicts_with = "activity",
        conflicts_with = "offset",
        default_value_t = false
    )]
    server: bool,

    #[command(flatten)]
    idle: Idle,

    #[command(flatten)]
    offset: Offset,
}

#[derive(Parser)]
#[group(required = false, multiple = false)]
struct Idle {
    /// Idle Idle
    #[arg(
        short,
        group = "activity",
        conflicts_with = "server",
        default_value_t = false
    )]
    idle: bool,

    /// Not Idle Idle
    #[arg(
        short,
        group = "activity",
        conflicts_with = "server",
        default_value_t = false
    )]
    active: bool,
}

#[derive(Parser)]
#[group(required = false, multiple = false)]
struct Offset {
    /// Increase
    #[arg(
        long,
        group = "offset",
        conflicts_with = "server",
        default_value = None
    )]
    increase: Option<i8>,

    /// Decrease
    #[arg(
        long,
        group = "offset",
        conflicts_with = "server",
        default_value = None
    )]
    decrease: Option<i8>,
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
    command_receiver: Receiver<Command>,
}

impl<'a> AmbientBrightnessController<'a> {
    fn create(close_receiver: Receiver<()>, command_receiver: Receiver<Command>) -> Result<Self> {
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
                    info!("Received Shutdown");
                    break
                },
                recv(self.borrow_command_receiver()) -> msg => match msg {
                    Err(e) => {
                        info!("Command Channel Terminated: {:#}", e);
                        break;
                    },
                    Ok(msg) => match msg {
                        Command::Idle => {
                            self.with_ambient_brightness_mut(|x| x.idle());
                            self.update()?
                        },

                        Command::Active => {
                            self.with_ambient_brightness_mut(|x| x.active());
                            self.update()?
                        },
                        Command::Increase(amount) => {
                            self.with_screen_brightness_mut(|x| x.increase(amount));
                            self.update()?
                        },
                        Command::Decrease(amount) => {
                            self.with_screen_brightness_mut(|x| x.decrease(amount));
                            self.update()?
                        }
                    },
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
    let exit_bool = Arc::new(AtomicBool::new(false));
    let (close_sender, close_receiver) = bounded(1);

    let exit_bool1 = exit_bool.clone();
    ctrlc::set_handler(move || {
        exit_bool1.store(true, atomic::Ordering::Relaxed);
        close_sender
            .send(())
            .expect("Could not send signal on channel.")
    })
    .context("Error setting Ctrl-C handler")?;

    let args = Args::parse();

    if args.server {
        let (control_server, command_receiver) = ControlServer::new()?;
        let ambient_brightness_controller =
            AmbientBrightnessController::create(close_receiver, command_receiver)?;

        let join_handle = control_server.run(exit_bool.clone());
        ambient_brightness_controller.run()?;

        info!("Waiting for Server Thread to stop.");
        join_handle
            .join()
            .map_err(|e| anyhow!("Error waiting for Server Thread: {:?}", e))??;
    } else {
        let mut client = ControlClient::new()?;

        if args.idle.idle {
            client.idle()?;
        }
        if args.idle.active {
            client.active()?;
        }

        if let Some(amount) = args.offset.increase {
            client.increase(amount)?;
        }
        if let Some(amount) = args.offset.decrease {
            client.decrease(amount)?;
        }

        info!("Done");
    }

    Ok(())
}
