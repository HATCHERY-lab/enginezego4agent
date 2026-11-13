// engine4agent remaining host protocols. No I/O.

//! Provider slot identity.
//!
//! The slot is a static id. No provider session is opened.

/// A provider slot id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSlot {
    pub id: &'static str,
}

/// The engine4agent provider slot.
pub const E4A_SLOT: ProviderSlot = ProviderSlot { id: "e4a" };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_id_is_e4a() {
        assert_eq!(E4A_SLOT.id, "e4a");
        assert_eq!(E4A_SLOT, ProviderSlot { id: "e4a" });
    }
}
