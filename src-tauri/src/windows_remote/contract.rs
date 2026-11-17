const fn same(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
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
        include_str!("../remote.rs"),
        include_str!("linux_remote.reference")
    ),
    "Linux remote contract changed: review Windows adapter before updating reference"
);
