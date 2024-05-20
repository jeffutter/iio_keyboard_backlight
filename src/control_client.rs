use std::{env, io::Write, os::unix::net::UnixStream, path::Path};

use anyhow::Result;
use byteorder::WriteBytesExt;

pub struct ControlClient {
    client: UnixStream,
}

impl ControlClient {
    pub fn new() -> Result<Self> {
        let socket_path = Path::new(&env::temp_dir()).join("ambient_brightness.sock");
        let client = std::os::unix::net::UnixStream::connect(socket_path)?;

        Ok(Self { client })
    }

    pub fn dim(&mut self) -> Result<()> {
        self.client.write_u8(0)?;
        self.client.flush()?;
        Ok(())
    }

    pub fn undim(&mut self) -> Result<()> {
        self.client.write_u8(1)?;
        self.client.flush()?;
        Ok(())
    }
}
