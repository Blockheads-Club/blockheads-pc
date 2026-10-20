//! Host sockets are not available on this platform yet: the guest keeps bionic's failing socket stubs.

use super::Vfd;
use bhpc_bionic::Shared;
use bhpc_cpu::Cpu;
use std::time::Duration;

pub(super) const SLICE: Duration = Duration::from_millis(5);

/// Never constructed here.
pub(super) struct HostSock;

impl HostSock {
    pub(super) fn ready(&self) -> u32 {
        0
    }

    pub(super) fn pending(&self) -> u32 {
        0
    }
}

/// No guest sockets here, so no LAN play either.
pub fn host_has_lan() -> bool {
    false
}

impl Vfd {
    pub(super) fn install_net(&self, _cpu: &Cpu, _st: &Shared) {}

    pub(super) fn net_read(&self, _c: &mut Cpu, _st: &Shared, _fd: u32, _buf: u32, _n: u32) -> bool {
        false
    }

    pub(super) fn net_write(&self, _c: &mut Cpu, _st: &Shared, _fd: u32, _buf: u32, _n: u32) -> bool {
        false
    }
}
