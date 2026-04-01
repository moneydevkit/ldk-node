//! Monotonic counters for HTLC forward outcomes involving LSP client channels.
//!
//! Only forwards where at least one side is a private (unannounced) channel are counted.
//! Network-to-network routing traffic is ignored entirely.

use std::sync::atomic::{AtomicU64, Ordering};

/// The direction of a forward relative to the LSP's client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForwardDirection {
	/// Incoming payment to a client (network → private channel).
	ToClient,
	/// Outgoing payment from a client (private channel → network).
	FromClient,
}

/// Monotonic counters for HTLC forward outcomes involving client channels.
///
/// Dimensions:
/// - **Direction**: `to_client` (inbound to client) vs `from_client` (client sending out).
/// - **Outcome**: `success` vs `failure`.
/// - **Failure source** (failures only): `downstream` (peer failed it back) vs `local` (we rejected).
///
/// Network-to-network forwards (both channels public) are not counted.
///
/// These are plain atomics so ldk-node stays free of any metrics framework dependency.
/// The application layer reads them via [`ForwardCounters::load_all`] and maps to prometheus.
#[derive(Default, Debug)]
pub struct ForwardCounters {
	/// Successful forwards to a client's private channel.
	pub success_to_client: AtomicU64,
	/// Successful forwards from a client's private channel to the network.
	pub success_from_client: AtomicU64,
	/// Failed forwards to a client's private channel (downstream failure).
	pub failure_to_client_downstream: AtomicU64,
	/// Failed forwards to a client's private channel (local failure).
	pub failure_to_client_local: AtomicU64,
	/// Failed forwards from a client's private channel to the network (downstream failure).
	pub failure_from_client_downstream: AtomicU64,
	/// Failed forwards from a client's private channel to the network (local failure).
	pub failure_from_client_local: AtomicU64,
	/// HTLC referenced an SCID we don't recognize (UnknownNextHop / InvalidForward).
	/// Likely a bug in private channel alias tracking.
	pub failure_invalid_forward_scid: AtomicU64,
}

impl ForwardCounters {
	/// Creates a new set of zeroed counters.
	pub fn new() -> Self {
		Self::default()
	}

	/// Classifies a forward by whether it involves a client channel and in which direction.
	/// Payments between clients are classified as toward the client.
	///
	/// Returns `None` for network-to-network forwards (both channels public).
	/// Defaults to private if a channel is not found (e.g. already closed).
	pub(crate) fn classify(
		channels: &[lightning::ln::channel_state::ChannelDetails],
		prev_channel_id: &lightning::ln::types::ChannelId,
		next_channel_id: &lightning::ln::types::ChannelId,
	) -> Option<ForwardDirection> {
		let prev_private = channels
			.iter()
			.find(|c| c.channel_id == *prev_channel_id)
			.map_or(true, |c| !c.is_announced);
		let next_private = channels
			.iter()
			.find(|c| c.channel_id == *next_channel_id)
			.map_or(true, |c| !c.is_announced);

		match (prev_private, next_private) {
			(_, true) => Some(ForwardDirection::ToClient),
			(true, false) => Some(ForwardDirection::FromClient),
			(false, false) => None,
		}
	}

	pub(crate) fn record_success(&self, direction: ForwardDirection) {
		match direction {
			ForwardDirection::ToClient => {
				self.success_to_client.fetch_add(1, Ordering::Relaxed);
			},
			ForwardDirection::FromClient => {
				self.success_from_client.fetch_add(1, Ordering::Relaxed);
			},
		}
	}

	pub(crate) fn record_failure(
		&self, direction: ForwardDirection, is_downstream: bool,
	) {
		match (direction, is_downstream) {
			(ForwardDirection::ToClient, true) => {
				self.failure_to_client_downstream.fetch_add(1, Ordering::Relaxed);
			},
			(ForwardDirection::ToClient, false) => {
				self.failure_to_client_local.fetch_add(1, Ordering::Relaxed);
			},
			(ForwardDirection::FromClient, true) => {
				self.failure_from_client_downstream.fetch_add(1, Ordering::Relaxed);
			},
			(ForwardDirection::FromClient, false) => {
				self.failure_from_client_local.fetch_add(1, Ordering::Relaxed);
			},
		}
	}

	pub(crate) fn record_invalid_scid(&self) {
		self.failure_invalid_forward_scid.fetch_add(1, Ordering::Relaxed);
	}

	/// Takes a point-in-time snapshot of all counter values.
	pub fn load_all(&self) -> ForwardSnapshot {
		ForwardSnapshot {
			success_to_client: self.success_to_client.load(Ordering::Relaxed),
			success_from_client: self.success_from_client.load(Ordering::Relaxed),
			failure_to_client_downstream: self.failure_to_client_downstream.load(Ordering::Relaxed),
			failure_to_client_local: self.failure_to_client_local.load(Ordering::Relaxed),
			failure_from_client_downstream: self
				.failure_from_client_downstream
				.load(Ordering::Relaxed),
			failure_from_client_local: self.failure_from_client_local.load(Ordering::Relaxed),
			failure_invalid_forward_scid: self
				.failure_invalid_forward_scid
				.load(Ordering::Relaxed),
		}
	}
}

/// Point-in-time snapshot of all forward counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct ForwardSnapshot {
	/// Successful forwards to a client's private channel.
	pub success_to_client: u64,
	/// Successful forwards from a client's private channel to the network.
	pub success_from_client: u64,
	/// Failed forwards to a client's private channel (downstream failure).
	pub failure_to_client_downstream: u64,
	/// Failed forwards to a client's private channel (local failure).
	pub failure_to_client_local: u64,
	/// Failed forwards from a client's private channel to the network (downstream failure).
	pub failure_from_client_downstream: u64,
	/// Failed forwards from a client's private channel to the network (local failure).
	pub failure_from_client_local: u64,
	/// HTLC referenced an SCID we don't recognize.
	pub failure_invalid_forward_scid: u64,
}

#[cfg(test)]
mod tests {
	use super::*;
	use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
	use lightning::ln::channel_state::{ChannelCounterparty, ChannelDetails};
	use lightning::ln::types::ChannelId;
	use lightning_types::features::InitFeatures;

	fn dummy_pubkey() -> PublicKey {
		let secp = Secp256k1::new();
		PublicKey::from_secret_key(&secp, &SecretKey::from_slice(&[1u8; 32]).unwrap())
	}

	fn make_channel(id: [u8; 32], announced: bool) -> ChannelDetails {
		ChannelDetails {
			channel_id: ChannelId::from_bytes(id),
			counterparty: ChannelCounterparty {
				node_id: dummy_pubkey(),
				features: InitFeatures::empty(),
				unspendable_punishment_reserve: 0,
				forwarding_info: None,
				outbound_htlc_minimum_msat: None,
				outbound_htlc_maximum_msat: None,
			},
			funding_txo: None,
			channel_type: None,
			short_channel_id: None,
			outbound_scid_alias: None,
			inbound_scid_alias: None,
			channel_value_satoshis: 1_000_000,
			unspendable_punishment_reserve: None,
			user_channel_id: 0,
			feerate_sat_per_1000_weight: None,
			outbound_capacity_msat: 0,
			next_outbound_htlc_limit_msat: 0,
			next_outbound_htlc_minimum_msat: 0,
			inbound_capacity_msat: 0,
			confirmations_required: None,
			confirmations: None,
			force_close_spend_delay: None,
			is_outbound: false,
			is_channel_ready: true,
			channel_shutdown_state: None,
			is_usable: true,
			is_announced: announced,
			inbound_htlc_minimum_msat: None,
			inbound_htlc_maximum_msat: None,
			config: None,
			pending_inbound_htlcs: vec![],
			pending_outbound_htlcs: vec![],
			funding_redeem_script: None,
		}
	}

	fn cid(b: u8) -> ChannelId {
		ChannelId::from_bytes([b; 32])
	}

	#[test]
	fn classify_network_to_client() {
		let channels = vec![
			make_channel([1; 32], true),
			make_channel([2; 32], false),
		];
		assert_eq!(
			ForwardCounters::classify(&channels, &cid(1), &cid(2)),
			Some(ForwardDirection::ToClient),
		);
	}

	#[test]
	fn classify_client_to_network() {
		let channels = vec![
			make_channel([1; 32], false),
			make_channel([2; 32], true),
		];
		assert_eq!(
			ForwardCounters::classify(&channels, &cid(1), &cid(2)),
			Some(ForwardDirection::FromClient),
		);
	}

	#[test]
	fn classify_network_to_network_ignored() {
		let channels = vec![
			make_channel([1; 32], true),
			make_channel([2; 32], true),
		];
		assert_eq!(ForwardCounters::classify(&channels, &cid(1), &cid(2)), None);
	}

	#[test]
	fn classify_client_to_client_is_to_client() {
		let channels = vec![
			make_channel([1; 32], false),
			make_channel([2; 32], false),
		];
		assert_eq!(
			ForwardCounters::classify(&channels, &cid(1), &cid(2)),
			Some(ForwardDirection::ToClient),
		);
	}

	#[test]
	fn classify_unknown_channel_defaults_to_private() {
		// next_channel_id not in list -> treated as private -> ToClient
		let channels = vec![make_channel([1; 32], true)];
		assert_eq!(
			ForwardCounters::classify(&channels, &cid(1), &cid(99)),
			Some(ForwardDirection::ToClient),
		);
	}

	#[test]
	fn record_success_increments_correct_counter() {
		let c = ForwardCounters::new();
		c.record_success(ForwardDirection::ToClient);
		c.record_success(ForwardDirection::ToClient);
		c.record_success(ForwardDirection::FromClient);

		let snap = c.load_all();
		assert_eq!(snap.success_to_client, 2);
		assert_eq!(snap.success_from_client, 1);
	}

	#[test]
	fn record_failure_increments_correct_counter() {
		let c = ForwardCounters::new();
		c.record_failure(ForwardDirection::ToClient, true);
		c.record_failure(ForwardDirection::ToClient, false);
		c.record_failure(ForwardDirection::FromClient, true);
		c.record_failure(ForwardDirection::FromClient, false);
		c.record_failure(ForwardDirection::ToClient, true);

		let snap = c.load_all();
		assert_eq!(snap.failure_to_client_downstream, 2);
		assert_eq!(snap.failure_to_client_local, 1);
		assert_eq!(snap.failure_from_client_downstream, 1);
		assert_eq!(snap.failure_from_client_local, 1);
		// successes untouched
		assert_eq!(snap.success_to_client, 0);
	}

	#[test]
	fn record_unknown_scid_increments() {
		let c = ForwardCounters::new();
		c.record_invalid_scid();
		c.record_invalid_scid();
		c.record_invalid_scid();

		let snap = c.load_all();
		assert_eq!(snap.failure_invalid_forward_scid, 3);
		assert_eq!(snap.success_to_client, 0);
	}
}

