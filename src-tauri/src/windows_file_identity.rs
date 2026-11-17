//! Ephemeral handle observations. Never serialized or treated as global IDs.
use sha2::{Digest, Sha256};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FileIdentity {
    Id128([u8; 24]),
    Smb64 { volume: u32, index: u64 },
    #[cfg(not(windows))]
    Portable([u8; 24]),
}
impl FileIdentity {
    pub(super) fn smb64(volume: u32, high: u32, low: u32) -> Option<Self> {
        let index = ((high as u64) << 32) | low as u64;
        (index != 0).then_some(Self::Smb64 { volume, index })
    }
    pub(super) fn update_revision(self, hash: &mut Sha256) {
        match self {
            Self::Id128(bytes) => hash.update(bytes), // preserve existing revisions
            Self::Smb64 { volume, index } => {
                hash.update(b"windows-smb-id64-v1\0");
                hash.update(volume.to_le_bytes());
                hash.update(index.to_le_bytes());
            }
            #[cfg(not(windows))]
            Self::Portable(bytes) => hash.update(bytes),
        }
    }
}
pub(super) fn legacy_query_allowed(error: u32) -> bool { matches!(error, 1 | 50 | 87) }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn width_and_volume_are_part_of_identity_not_padded_into_a_128bit_id() {
        let id = FileIdentity::smb64(7, 1, 9).unwrap();
        assert_ne!(id, FileIdentity::smb64(8, 1, 9).unwrap());
        assert_ne!(id, FileIdentity::smb64(7, 0, 9).unwrap());
        let mut padded = [0;24]; padded[..8].copy_from_slice(&7u64.to_le_bytes());
        padded[8..16].copy_from_slice(&((1u64<<32)|9).to_le_bytes());
        assert_ne!(id, FileIdentity::Id128(padded));
        assert!(FileIdentity::smb64(7,0,0).is_none());
    }
    #[test]
    fn access_disconnect_and_memory_failures_do_not_select_another_identity_api() {
        for code in [0,5,32,53,64,67,123,998,995] { assert!(!legacy_query_allowed(code)); }
        for code in [1,50,87] { assert!(legacy_query_allowed(code)); }
    }
    #[test]
    fn legacy_revision_input_is_preserved() {
        let bytes=[7;24]; let mut old=Sha256::new();old.update(bytes);
        let mut current=Sha256::new();FileIdentity::Id128(bytes).update_revision(&mut current);
        assert_eq!(old.finalize(),current.finalize());
        let mut narrow=Sha256::new();FileIdentity::smb64(7,0,7).unwrap().update_revision(&mut narrow);
        let mut wide=Sha256::new();FileIdentity::Id128(bytes).update_revision(&mut wide);
        assert_ne!(narrow.finalize(),wide.finalize());
    }
}
