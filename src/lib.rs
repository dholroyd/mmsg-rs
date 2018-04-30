extern crate libc;
extern crate iovec;
#[macro_use]
extern crate bitflags;

use std::net;
use std::io;
use std::cmp;
use libc::{c_int, ssize_t, msghdr, mmsghdr, timespec, MSG_DONTWAIT, MSG_CMSG_CLOEXEC,MSG_ERRQUEUE, MSG_PEEK, MSG_TRUNC, MSG_WAITFORONE};
use std::os::unix::io::AsRawFd;
use std::marker::PhantomData;
use std::time;

bitflags! {
    #[derive(Default)]
    pub struct MsgFlags: c_int {
        const DONTWAIT      = MSG_DONTWAIT;
        const CMSG_CLOEXEC  = MSG_CMSG_CLOEXEC;
        const ERRQUEUE      = MSG_ERRQUEUE;
        const PEEK          = MSG_PEEK;
        const TRUNC         = MSG_TRUNC;
        const WAITFORONE    = MSG_WAITFORONE;
    }
}

#[repr(C)]
pub struct MMsgHdr<'buf> {
    hdr: mmsghdr,
    phantom: PhantomData<&'buf ()>,
}
impl<'buf> MMsgHdr<'buf> {
   pub fn new(iovec: &mut[&'buf mut iovec::IoVec], flags: MsgFlags) -> MMsgHdr<'buf> {
        let vlen = iovec.len();
        // TODO:
        //  - support 'control'
        //  - support 'name'
        MMsgHdr {
            hdr: mmsghdr {
                msg_hdr: msghdr {
                    msg_control: std::ptr::null_mut(),
                    msg_controllen: 0,
                    msg_flags: flags.bits(),
                    msg_iov: iovec::unix::as_os_slice_mut(iovec).as_mut_ptr(),
                    msg_iovlen: vlen,
                    msg_name: std::ptr::null_mut(),
                    msg_namelen: 0,
                },
                msg_len: 0,
            },
            phantom: PhantomData,
        }
    }

    fn msg_len(&self) -> usize {
        self.hdr.msg_len as usize
    }
}

/// Methods will panic if given a timeout value that will not fit into the system `timespec` type
trait MMsg {
    fn recvmmsg(&self, msgvec: &mut[MMsgHdr], flags: MsgFlags, timeout: Option<time::Duration>) -> io::Result<usize>;
    fn sendmmsg(&self, msgvec: &mut[MMsgHdr]) -> io::Result<usize>;
}

impl MMsg for net::UdpSocket {
    fn recvmmsg(&self, msgvec: &mut[MMsgHdr], flags: MsgFlags, timeout: Option<time::Duration>) -> io::Result<usize> {
        let len = cmp::min(msgvec.len(), max_len()) as u32;
        let mut t = timeout.map(|d| timespec {
            tv_sec: d.as_secs() as i64,
            tv_nsec: d.subsec_nanos() as i64,
        });
        let tptr = match t {
            Some(ref mut time) => time,
            None => std::ptr::null_mut(),
        };
        unsafe {
            let n = cvt({
                libc::recvmmsg(
                    self.as_raw_fd(),
                    msgvec.as_mut_ptr() as *mut mmsghdr,
                    len,
                    flags.bits(),
                    tptr,
                )
            })?;
            Ok(n as usize)
        }
    }
    fn sendmmsg(&self, msgvec: &mut[MMsgHdr]) -> io::Result<usize> {
        let len = cmp::min(msgvec.len(), max_len()) as u32;
        unsafe {
            let n = cvt({
                libc::sendmmsg(
                    self.as_raw_fd(),
                    msgvec.as_mut_ptr() as *mut mmsghdr,
                    len,
                    0,
                )
            })?;
            Ok(n as usize)
        }
    }
}

fn cvt(t: i32) -> io::Result<i32> {
    if t == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(t)
    }
}

fn max_len() -> usize {
    // The maximum read limit on most posix-like systems is `SSIZE_MAX`,
    // with the man page quoting that if the count of bytes to read is
    // greater than `SSIZE_MAX` the result is "unspecified".
    //
    // On macOS, however, apparently the 64-bit libc is either buggy or
    // intentionally showing odd behavior by rejecting any read with a size
    // larger than or equal to INT_MAX. To handle both of these the read
    // size is capped on both platforms.
    if cfg!(target_os = "macos") {
        <c_int>::max_value() as usize - 1
    } else {
        <ssize_t>::max_value() as usize
    }
}

#[cfg(test)]
mod tests {
    use std::{thread, time};
    use std::net::UdpSocket;
    use super::*;

    #[test]
    fn it_works() {
        let sender = thread::spawn(|| {
            let so = UdpSocket::bind("127.0.0.1:0")
                .unwrap();
            so.connect("127.0.0.1:3456").unwrap();
            let mut a = [b'A'; 500];
            let mut b = [b'B'; 500];
            let mut c = [b'C'; 500];
            let mut iov_a = [ (&mut a[..]).into() ];
            let mut iov_b = [ (&mut b[..]).into() ];
            let mut iov_c = [ (&mut c[..]).into() ];
            let mut msgs = [
                MMsgHdr::new(&mut iov_a[..], MsgFlags::default()),
                MMsgHdr::new(&mut iov_b[..], MsgFlags::default()),
                MMsgHdr::new(&mut iov_c[..], MsgFlags::default()),
            ];
            thread::park();
            so.sendmmsg(&mut msgs[..]).unwrap();
        });
        let so = UdpSocket::bind("127.0.0.1:3456")
            .unwrap();
        sender.thread().unpark();
        // this is not the proper way to coordinate with the sender thread!
        thread::sleep(time::Duration::from_millis(200));
        let mut a = [0u8; 1500];
        let mut b = [0u8; 1500];
        let mut c = [0u8; 1500];
        let mut iov_a = [ (&mut a[..]).into() ];
        let mut iov_b = [ (&mut b[..]).into() ];
        let mut iov_c = [ (&mut c[..]).into() ];
        let mut msgs = [
            MMsgHdr::new(&mut iov_a[..], MsgFlags::default()),
            MMsgHdr::new(&mut iov_b[..], MsgFlags::default()),
            MMsgHdr::new(&mut iov_c[..], MsgFlags::default()),
        ];
        let count = so.recvmmsg(&mut msgs[..], MsgFlags::DONTWAIT, None).unwrap();
        assert_eq!(3, count);
        assert_eq!(500, msgs[0].msg_len());
        sender.join().unwrap();
    }
}
