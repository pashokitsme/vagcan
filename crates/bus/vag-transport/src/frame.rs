#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanId {
	Standard(u16),
	Extended(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanFrame {
	pub id: CanId,
	pub data: Vec<u8>,
}

impl CanFrame {
	pub fn new(id: CanId, data: Vec<u8>) -> Self {
		assert!(data.len() <= 8, "classic CAN frame data must be <= 8 bytes");
		CanFrame { id, data }
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn frame_holds_id_and_data() {
		let f = CanFrame::new(CanId::Standard(0x7E0), vec![0x02, 0x10, 0x03]);
		assert_eq!(f.id, CanId::Standard(0x7E0));
		assert_eq!(f.data, vec![0x02, 0x10, 0x03]);
	}

	#[test]
	#[should_panic(expected = "must be <= 8 bytes")]
	fn frame_rejects_oversized_data() {
		CanFrame::new(CanId::Standard(0x7E0), vec![0; 9]);
	}
}
