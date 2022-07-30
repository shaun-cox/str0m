use std::io;
use std::net::SocketAddr;
use std::slice;

use crate::UDP_MTU;

use super::inout::{NetworkOutput, NetworkOutputWriter};
use super::OutputQueue;

/// Helper to enqueue network output data.
pub(crate) struct OutputEnqueuer(SocketAddr, *mut OutputQueue);

impl OutputEnqueuer {
    /// SAFETY: The user of this enqueuer must guarantee that the
    /// instance does not outlive the lifetime of `&mut OutputQueue`.
    pub unsafe fn new(addr: SocketAddr, output: &mut OutputQueue) -> Self {
        OutputEnqueuer(addr, output as *mut OutputQueue)
    }

    pub fn get_buffer_writer(&mut self) -> NetworkOutputWriter {
        // SAFETY: See new
        let queue = unsafe { &mut *self.1 };

        queue.get_buffer_writer()
    }

    pub fn enqueue(&mut self, buffer: NetworkOutput) {
        // SAFETY: See new
        let queue = unsafe { &mut *self.1 };

        queue.enqueue(self.0, buffer);
    }
}

pub(crate) struct PtrBuffer {
    src: Option<(*const u8, usize)>,
    dst: Option<OutputEnqueuer>,
}

impl PtrBuffer {
    pub fn new() -> Self {
        PtrBuffer {
            src: None,
            dst: None,
        }
    }

    pub fn set_input(&mut self, src: &[u8]) {
        assert!(self.src.is_none());
        self.src = Some((src.as_ptr(), src.len()));
    }

    pub fn assert_input_was_read(&self) {
        assert!(self.src.is_none(), "PtrBuffer::src is not None");
    }

    pub fn set_output(&mut self, enqueuer: OutputEnqueuer) {
        self.dst = Some(enqueuer);
    }

    pub fn remove_output(&mut self) {
        self.dst = None;
    }
}

impl io::Read for PtrBuffer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some((ptr, len)) = self.src.take() {
            // SAFETY: this is only safe if the read() of this data is done in the same
            // scope calling set_read_src().
            let src = unsafe { slice::from_raw_parts(ptr, len) };

            // The read() call must read the entire buffer in one go, we can't fragment it.
            assert!(
                buf.len() >= len,
                "Read buf too small for entire PtrBuffer::src"
            );

            (&mut buf[0..len]).copy_from_slice(src);

            Ok(len)
        } else {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "WouldBlock"))
        }
    }
}

impl io::Write for PtrBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len();
        assert!(len <= UDP_MTU, "Too large DTLS packet: {}", buf.len());

        let enqueuer = self.dst.as_mut().expect("No set_output");
        let mut writer = enqueuer.get_buffer_writer();

        (&mut writer[0..buf.len()]).copy_from_slice(buf);
        let buffer = writer.set_len(buf.len());

        enqueuer.enqueue(buffer);

        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}