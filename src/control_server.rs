use std::{
    env, fs,
    path::Path,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::Result;
use byteorder::ReadBytesExt;
use crossbeam::channel::{bounded, Receiver, Sender};
use log::{debug, info};
use mio::{net::UnixListener, Events, Interest, Poll, Token};

pub struct ControlServer {
    poll: Poll,
    listener: UnixListener,
    command_sender: Sender<u8>,
}

impl ControlServer {
    pub fn new() -> Result<(Self, Receiver<u8>)> {
        let socket_path = Path::new(&env::temp_dir()).join("ambient_brightness.sock");
        fs::remove_file(socket_path.clone())?;
        let mut listener = UnixListener::bind(socket_path)?;
        let poll = Poll::new()?;
        poll.registry().register(
            &mut listener,
            Token(0),
            Interest::READABLE | Interest::WRITABLE,
        )?;
        let (command_sender, command_receiver) = bounded(1);

        Ok((
            Self {
                poll,
                listener,
                command_sender,
            },
            command_receiver,
        ))
    }

    pub fn run(mut self, exit_bool: Arc<AtomicBool>) -> JoinHandle<Result<()>> {
        thread::spawn(move || {
            let mut events = Events::with_capacity(1024);

            loop {
                if exit_bool.load(atomic::Ordering::Relaxed) {
                    info!("Control Server Shutting Down");
                    break;
                }

                self.poll
                    .poll(&mut events, Some(Duration::from_millis(100)))?;

                for event in &events {
                    debug!("Event: {:?}", event);
                    if event.token() == Token(0) && event.is_readable() {
                        let (mut socket, _addr) = self.listener.accept()?;
                        let socket_read = socket.read_u8()?;
                        debug!("Got Message: {}", socket_read);

                        match socket_read {
                            0 => self.command_sender.send(0)?,
                            1 => self.command_sender.send(1)?,
                            _ => (),
                        }
                    }
                }
            }

            Ok(())
        })
    }
}
