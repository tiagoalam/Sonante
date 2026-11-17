//! Build-time drift gate for the original Linux calculation copied into the
//! separately audited Windows consumer. This is not an artifact trust hash.
const fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 14695981039346656037u64;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(1099511628211);
        i += 1;
    }
    hash
}
const _: () = assert!(
    fingerprint(include_bytes!("../analyzer.rs")) == 972956366489690586,
    "Windows analyzer source must be reviewed against this Linux revision"
);

const _: () = assert!(
    fingerprint(include_bytes!("linux_policy.rs")) == 4967379610799718436,
    "Windows DSD policy copy must retain the reviewed Linux contract"
);
