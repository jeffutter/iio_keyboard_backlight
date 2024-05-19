mod ambient_brightness;
mod kbd_brightness;
mod screen_brightness;

use std::{
    env, fs,
    io::Write,
    path::Path,
    sync::atomic::{self, AtomicBool},
    thread,
    time::Duration,
};

use ambient_brightness::AmbientBrightness;
use anyhow::{anyhow, Result};
use clap::Parser;
use crossbeam::{
    channel::{after, bounded, tick},
    select,
};
use env_logger::Env;
use kbd_brightness::KBDBrightness;
use log::{debug, info, trace};
use logind_zbus::session::SessionProxyBlocking;
use mio::{net::UnixListener, Events, Interest, Poll, Token};
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

fn update(
    ambient_brightness: &mut AmbientBrightness,
    kbd_brightness: &mut KBDBrightness,
    screen_brightness: &mut ScreenBrightness,
) -> Result<()> {
    let new_val = ambient_brightness.update()?;
    trace!("New Val POST: {}", new_val);
    kbd_brightness.adjust(new_val)?;
    screen_brightness.adjust(new_val)?;
    Ok(())
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
        let connection = Connection::system()?;
        let proxy = SessionProxyBlocking::builder(&connection)
            .path("/org/freedesktop/login1/session/auto")?
            .build()?;

        let mut ambient_brightness = AmbientBrightness::new()?.init()?;
        let mut kbd_brightness = KBDBrightness::new(&proxy, "leds", "asus::kbd_backlight");
        let mut screen_brightness = ScreenBrightness::new(&proxy, "backlight", "intel_backlight")?;
        let (command_sender, command_receiver) = bounded(1);

        let join_handle: thread::JoinHandle<Result<()>> = thread::spawn(move || {
            let socket_path = Path::new(&env::temp_dir()).join("ambient_brightness.sock");
            fs::remove_file(socket_path.clone())?;
            let mut listener = UnixListener::bind(socket_path)?;
            let mut poll = Poll::new()?;
            let mut events = Events::with_capacity(1024);
            poll.registry().register(
                &mut listener,
                Token(0),
                Interest::READABLE | Interest::WRITABLE,
            )?;

            loop {
                if exit_bool1.load(atomic::Ordering::Relaxed) {
                    break;
                }
                poll.poll(&mut events, Some(Duration::from_millis(100)))?;

                for event in &events {
                    debug!("Event: {:?}", event);
                    if event.token() == Token(0) && event.is_readable() {
                        let (mut socket, _addr) = listener.accept()?;
                        let socket_read = socket.read_u8(Endian::native())?;
                        debug!("Got Message: {}", socket_read);

                        match socket_read {
                            0 => command_sender.send(0)?,
                            1 => command_sender.send(1)?,
                            _ => (),
                        }
                    }
                }
            }

            info!("Server Thread Exiting");
            Ok(())
        });

        let ticker = tick(Duration::from_secs(5));
        let first = after(Duration::from_millis(1));

        loop {
            select! {
                recv(close_receiver) -> _ => {
                    break
                },
                recv(command_receiver) -> msg => match msg? {
                    0 => {
                        ambient_brightness.dim();
                        update(&mut ambient_brightness, &mut kbd_brightness, &mut screen_brightness)?
                    },

                    1 => {
                        ambient_brightness.undim();
                        update(&mut ambient_brightness, &mut kbd_brightness, &mut screen_brightness)?
                    },
                    _ => ()
                },
                recv(ticker) -> _  => {
                        update(&mut ambient_brightness, &mut kbd_brightness, &mut screen_brightness)?
                },
                recv(first) -> _ => {
                        update(&mut ambient_brightness, &mut kbd_brightness, &mut screen_brightness)?
                },
            }
        }

        info!("Waiting for Server Thread to stop.");
        join_handle
            .join()
            .map_err(|e| anyhow!("Error waiting for Server Thread: {:?}", e))??;
    } else {
        let socket_path = Path::new(&env::temp_dir()).join("ambient_brightness.sock");
        let mut client = std::os::unix::net::UnixStream::connect(socket_path)?;
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
