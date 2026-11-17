const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}
const _: () = assert!(
    same(
        include_str!("../upnp.rs"),
        include_str!("linux_upnp.reference")
    ),
    "Review Windows UPnP after Linux contract changes"
);
const _: () = assert!(
    same(
        include_str!("../upnp_protocol.rs"),
        include_str!("linux_protocol.reference")
    ),
    "Review Windows UPnP protocol after Linux changes"
);
