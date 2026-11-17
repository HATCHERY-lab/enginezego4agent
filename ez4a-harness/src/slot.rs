// engine zero for agents remaining host protocols. No I/O.

//! Provider slot identity.
//!
//! The slot is a static id. No provider session is opened.

/// A provider slot id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSlot {
    pub id: &'static str,
}

/// The engine zero for agents provider slot.
pub const EZ4A_SLOT: ProviderSlot = ProviderSlot { id: "ez4a" };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_id_is_ez4a() {
        assert_eq!(EZ4A_SLOT.id, "ez4a");
        assert_eq!(EZ4A_SLOT, ProviderSlot { id: "ez4a" });
    }
}
