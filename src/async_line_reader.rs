use std::net::TcpStream;
use async_ssh2_lite::AsyncStream;
use smol::{Async, io::AsyncReadExt};
use anyhow::Result;

const INITIAL_CAPACITY: usize = 1024;

pub struct AsyncLineReader {
    stream: AsyncStream<Async<TcpStream>>,
    line_buffer: Vec<u8>,
    buffer_size: usize
}

impl AsyncLineReader {
    pub fn new(stream: AsyncStream<Async<TcpStream>>) -> Self {
        Self {
            stream,
            line_buffer: vec![0u8; INITIAL_CAPACITY],
            buffer_size: 0
        }
    }

    pub async fn read_line(&mut self) -> Result<Vec<u8>> {
        let delim_pos = self.line_buffer[0..self.buffer_size]
            .iter()
            .position(|c| *c == b'\n');

        if let Some(pos) = delim_pos {
            let line = self.line_buffer[0..pos].to_vec();

            self.line_buffer.copy_within((pos+1)..self.buffer_size, 0);
            self.buffer_size -= pos;
            self.buffer_size -= 1;

            return Ok(line);
        }

        loop {
            // Double line buffer size if at capacity
            if self.buffer_size == self.line_buffer.len() {
                for _ in 0..=self.line_buffer.len() {
                    self.line_buffer.push(0u8);
                }
            }

            let n = self.stream.read(&mut self.line_buffer[self.buffer_size..]).await?;

            let delim_pos = self.line_buffer[self.buffer_size..(self.buffer_size+n)]
                .iter()
                .position(|c| *c == b'\n')
                .map(|pos| pos + self.buffer_size);

            self.buffer_size += n;
            if let Some(pos) = delim_pos {
                let line = self.line_buffer[0..pos].to_vec();

                self.line_buffer.copy_within((pos+1)..self.buffer_size, 0);
                self.buffer_size -= pos;
                self.buffer_size -= 1;

                return Ok(line);
            }
        }
    }
}