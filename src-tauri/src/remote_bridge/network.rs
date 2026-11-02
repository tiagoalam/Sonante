//! Addresses advertised by the web remote; UPnP discovery stays independent.
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CStr,
    io::Read,
    net::Ipv4Addr,
    path::Path,
};
#[derive(Clone)]
struct Interface {
    name: String,
    address: Ipv4Addr,
    bridge: bool,
    lan_uplink: bool,
}
fn default_routes(table: &str) -> BTreeMap<String, u32> {
    let mut routes = BTreeMap::new();
    for line in table.lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 8 || fields[1] != "00000000" || fields[7] != "00000000" {
            continue;
        }
        let Some(flags) = u32::from_str_radix(fields[3], 16).ok() else {
            continue;
        };
        let Some(metric) = fields[6].parse::<u32>().ok() else {
            continue;
        };
        if flags & 1 == 0 || flags & 0x200 != 0 {
            continue;
        }
        routes
            .entry(fields[0].to_owned())
            .and_modify(|old: &mut u32| *old = (*old).min(metric))
            .or_insert(metric);
    }
    routes
}
fn virtual_network(name: &str) -> bool {
    [
        "virbr", "docker", "vboxnet", "veth", "cni", "flannel", "lxc", "podman", "br-",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}
fn select(interfaces: Vec<Interface>, routes: &BTreeMap<String, u32>) -> Vec<Ipv4Addr> {
    let mut candidates: Vec<_> = interfaces
        .into_iter()
        .filter(|i| {
            (i.address.is_private() || i.address.is_link_local())
                && (routes.contains_key(&i.name)
                    || i.lan_uplink
                    || !i.bridge && !virtual_network(&i.name))
        })
        .collect();
    candidates.sort_by_key(|i| (routes.get(&i.name).copied().unwrap_or(u32::MAX), i.address));
    let mut seen = BTreeSet::new();
    let mut addresses: Vec<_> = candidates
        .into_iter()
        .map(|i| i.address)
        .filter(|ip| seen.insert(*ip))
        .collect();
    if addresses.is_empty() {
        addresses.push(Ipv4Addr::LOCALHOST);
    }
    addresses
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < libc::IFNAMSIZ
        && !matches!(name, "." | "..")
        && !name.contains('/')
}
// A LAN bridge may be the real host interface. Follow physical/VLAN/bond ports,
// rather than treating every Linux bridge or every 192.168.122.x IP as virtual.
fn lan_uplink(root: &Path, name: &str, remaining: u8) -> bool {
    if remaining == 0 || !safe_name(name) {
        return false;
    }
    let path = root.join(name);
    if path.join("device").exists() {
        return true;
    }
    for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
        if let Some(port) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.strip_prefix("lower_"))
            .map(str::to_owned)
        {
            if lan_uplink(root, &port, remaining - 1) {
                return true;
            }
        }
    }
    for entry in std::fs::read_dir(path.join("brif"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Some(port) = entry.file_name().to_str() {
            if lan_uplink(root, port, remaining - 1) {
                return true;
            }
        }
    }
    false
}
fn interfaces() -> Vec<Interface> {
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut item = head;
    let root = Path::new("/sys/class/net");
    while !item.is_null() {
        unsafe {
            let i = &*item;
            if !i.ifa_addr.is_null()
                && !i.ifa_name.is_null()
                && (*i.ifa_addr).sa_family as i32 == libc::AF_INET
                && i.ifa_flags & libc::IFF_UP as u32 != 0
                && i.ifa_flags & libc::IFF_MULTICAST as u32 != 0
            {
                if let Ok(name) = CStr::from_ptr(i.ifa_name).to_str() {
                    if safe_name(name) {
                        let ip = &*(i.ifa_addr as *const libc::sockaddr_in);
                        result.push(Interface {
                            name: name.into(),
                            address: Ipv4Addr::from(ip.sin_addr.s_addr.to_ne_bytes()),
                            bridge: root.join(name).join("bridge").is_dir(),
                            lan_uplink: lan_uplink(root, name, 4),
                        });
                    }
                }
            }
            item = i.ifa_next;
        }
    }
    unsafe { libc::freeifaddrs(head) };
    result
}
pub(super) fn addresses() -> Vec<Ipv4Addr> {
    let mut table = String::new();
    if let Ok(file) = std::fs::File::open("/proc/net/route") {
        let _ = file.take(65536).read_to_string(&mut table);
    }
    select(interfaces(), &default_routes(&table))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn iface(name: &str, ip: &str, bridge: bool, lan_uplink: bool) -> Interface {
        Interface {
            name: name.into(),
            address: ip.parse().unwrap(),
            bridge,
            lan_uplink,
        }
    }
    #[test]
    fn remote_address_list_excludes_vm_and_container_bridges_and_keeps_the_real_lan() {
        let interfaces = vec![
            iface("virbr0", "192.168.122.1", true, false),
            iface("enp5s0", "192.168.10.222", false, true),
            iface("docker0", "172.17.0.1", true, false),
            iface("vboxnet0", "192.168.56.1", false, false),
        ];
        assert_eq!(
            select(interfaces, &BTreeMap::from([("enp5s0".into(), 100)])),
            vec!["192.168.10.222".parse::<Ipv4Addr>().unwrap()]
        );
    }
    #[test]
    fn remote_real_lan_bridges_static_networks_and_guest_network_cards_remain_advertised() {
        for interface in [
            iface("br0", "192.168.50.10", true, true),
            iface("ens3", "10.0.2.15", false, true),
            iface("enp1s0", "192.168.122.8", false, true),
        ] {
            assert_eq!(
                select(vec![interface.clone()], &BTreeMap::new()),
                vec![interface.address]
            );
        }
        let default_bridge = iface("virbr-main", "10.20.0.1", true, false);
        assert_eq!(
            select(
                vec![default_bridge.clone()],
                &BTreeMap::from([("virbr-main".into(), 10)])
            ),
            vec![default_bridge.address]
        );
    }
    #[test]
    fn remote_default_route_metric_precedes_numeric_ip_order_and_duplicate_addresses_are_removed() {
        let interfaces = vec![
            iface("wifi", "10.0.0.9", false, true),
            iface("ethernet", "192.168.10.222", false, true),
            iface("ethernet", "192.168.10.222", false, true),
        ];
        let selected = select(
            interfaces,
            &BTreeMap::from([("ethernet".into(), 100), ("wifi".into(), 600)]),
        );
        assert_eq!(
            selected,
            vec![
                "192.168.10.222".parse::<Ipv4Addr>().unwrap(),
                "10.0.0.9".parse().unwrap()
            ]
        );
    }
    #[test]
    fn remote_route_parser_ignores_connected_routes_down_and_reject_routes_and_keeps_the_lowest_metric(
    ) {
        let table="Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\neth0 00000000 010AA8C0 0003 0 0 600 00000000 0 0 0\neth0 00000000 010AA8C0 0003 0 0 100 00000000 0 0 0\nvirbr0 007AA8C0 00000000 0001 0 0 0 00FFFFFF 0 0 0\ndown0 00000000 00000000 0000 0 0 0 00000000 0 0 0\nreject0 00000000 00000000 0201 0 0 0 00000000 0 0 0\n";
        assert_eq!(
            default_routes(table),
            BTreeMap::from([("eth0".into(), 100)])
        );
    }
    #[test]
    fn remote_no_lan_keeps_loopback_fallback_instead_of_advertising_an_isolated_bridge() {
        assert_eq!(
            select(
                vec![iface("virbr0", "192.168.122.1", true, false)],
                &BTreeMap::new()
            ),
            vec![Ipv4Addr::LOCALHOST]
        );
    }
}
