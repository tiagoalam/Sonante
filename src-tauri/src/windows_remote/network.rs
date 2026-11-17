use std::net::Ipv4Addr;
#[cfg(windows)]
fn read() -> Option<Vec<(u32, Ipv4Addr)>> {
    use windows_sys::Win32::{
        NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
        Networking::WinSock::*,
    };
    let mut storage = vec![0u64; 16384];
    let begin = storage.as_mut_ptr() as usize;
    let end = begin + storage.len() * 8;
    let mut length = (end - begin) as u32;
    let valid = |ptr: usize, size: usize| {
        ptr >= begin && ptr.checked_add(size).is_some_and(|n| n <= end) && ptr % 4 == 0
    };
    let head = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    if unsafe {
        GetAdaptersAddresses(
            AF_INET as u32,
            GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
            std::ptr::null(),
            head,
            &mut length,
        )
    } != 0
    {
        return None;
    }
    if length as usize > end - begin {
        return None;
    }
    let mut out = Vec::new();
    let mut adapter = head;
    let mut visited = std::collections::HashSet::new();
    while !adapter.is_null() {
        if visited.len() >= 128
            || !valid(
                adapter as usize,
                std::mem::size_of::<IP_ADAPTER_ADDRESSES_LH>(),
            )
            || !visited.insert(adapter as usize)
        {
            return None;
        }
        let a = unsafe { &*adapter };
        if a.OperStatus == IfOperStatusUp && a.IfType != 24 {
            let mut address = a.FirstUnicastAddress;
            let mut seen = std::collections::HashSet::new();
            while !address.is_null() {
                if seen.len() >= 128
                    || !valid(
                        address as usize,
                        std::mem::size_of::<IP_ADAPTER_UNICAST_ADDRESS_LH>(),
                    )
                    || !seen.insert(address as usize)
                {
                    return None;
                }
                let item = unsafe { &*address };
                let ptr = item.Address.lpSockaddr;
                if !ptr.is_null()
                    && item.Address.iSockaddrLength >= std::mem::size_of::<SOCKADDR_IN>() as i32
                    && valid(ptr as usize, std::mem::size_of::<SOCKADDR_IN>())
                    && unsafe { (*ptr).sa_family } == AF_INET
                {
                    let v = unsafe { &*(ptr.cast::<SOCKADDR_IN>()) };
                    let ip = Ipv4Addr::from(unsafe { v.sin_addr.S_un.S_addr }.to_ne_bytes());
                    if ip.is_private() || ip.is_link_local() {
                        out.push((a.Ipv4Metric, ip));
                    }
                }
                address = item.Next;
            }
        }
        adapter = a.Next;
    }
    Some(out)
}
pub(super) fn addresses() -> Vec<Ipv4Addr> {
    #[cfg(windows)]
    let mut addresses = read().unwrap_or_default();
    #[cfg(not(windows))]
    let mut addresses: Vec<(u32, Ipv4Addr)> = Vec::new();
    addresses.sort();
    let mut result = Vec::new();
    for (_, ip) in addresses {
        if !result.contains(&ip) {
            result.push(ip)
        }
    }
    if result.is_empty() {
        result.push(Ipv4Addr::LOCALHOST);
    }
    result
}
