use crate::client_state::ETHEREUM_CLIENT_REVISION_NUMBER;
use crate::commitment::decode_eip1184_rlp_proof;
use crate::errors::Error;
use crate::internal_prelude::*;
use ethereum_consensus::beacon::{BeaconBlockHeader, Slot};
use ethereum_consensus::bls::{PublicKey, Signature};
use ethereum_consensus::sync_protocol::{SyncAggregate, SyncCommittee};
use ethereum_consensus::types::{H256, U64};
use ethereum_ibc_proto::ibc::core::client::v1::Height as ProtoHeight;
use ethereum_ibc_proto::ibc::lightclients::ethereum::v1::{
    AccountUpdate as ProtoAccountUpdate, BeaconBlockHeader as ProtoBeaconBlockHeader,
    ConsensusUpdate as ProtoConsensusUpdate, ExecutionUpdate as ProtoExecutionUpdate,
    SyncAggregate as ProtoSyncAggregate, SyncCommittee as ProtoSyncCommittee,
    TrustedSyncCommittee as ProtoTrustedSyncCommittee,
};
use ethereum_light_client_verifier::updates::{ConsensusUpdate, ExecutionUpdate};
use ibc::Height;
use ssz_rs::{Bitvector, Deserialize, Vector};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConsensusUpdateInfo<const SYNC_COMMITTEE_SIZE: usize> {
    /// Header attested to by the sync committee
    pub attested_header: BeaconBlockHeader,
    /// Next sync committee contained in `attested_header.state_root`
    /// 0: sync committee
    /// 1: branch indicating the next sync committee in the tree corresponding to `attested_header.state_root`
    pub next_sync_committee: Option<(SyncCommittee<SYNC_COMMITTEE_SIZE>, Vec<H256>)>,
    /// Finalized header contained in `attested_header.state_root`
    /// 0: header
    /// 1. branch indicating the header in the tree corresponding to `attested_header.state_root`
    pub finalized_header: (BeaconBlockHeader, Vec<H256>),
    /// Sync committee aggregate signature
    pub sync_aggregate: SyncAggregate<SYNC_COMMITTEE_SIZE>,
    /// Slot at which the aggregate signature was created (untrusted)
    pub signature_slot: Slot,
    /// Execution payload contained in the finalized beacon block's body
    pub finalized_execution_root: H256,
    /// Execution payload branch indicating the payload in the tree corresponding to the finalized block's body
    pub finalized_execution_branch: Vec<H256>,
}

impl<const SYNC_COMMITTEE_SIZE: usize> ConsensusUpdate<SYNC_COMMITTEE_SIZE>
    for ConsensusUpdateInfo<SYNC_COMMITTEE_SIZE>
{
    fn attested_beacon_header(&self) -> &BeaconBlockHeader {
        &self.attested_header
    }
    fn next_sync_committee(&self) -> Option<&SyncCommittee<SYNC_COMMITTEE_SIZE>> {
        self.next_sync_committee.as_ref().map(|c| &c.0)
    }
    fn next_sync_committee_branch(&self) -> Option<Vec<H256>> {
        self.next_sync_committee.as_ref().map(|c| c.1.to_vec())
    }
    fn finalized_beacon_header(&self) -> &BeaconBlockHeader {
        &self.finalized_header.0
    }
    fn finalized_beacon_header_branch(&self) -> Vec<H256> {
        self.finalized_header.1.to_vec()
    }
    fn finalized_execution_root(&self) -> H256 {
        self.finalized_execution_root
    }
    fn finalized_execution_branch(&self) -> Vec<H256> {
        self.finalized_execution_branch.to_vec()
    }
    fn sync_aggregate(&self) -> &SyncAggregate<SYNC_COMMITTEE_SIZE> {
        &self.sync_aggregate
    }
    fn signature_slot(&self) -> Slot {
        self.signature_slot
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecutionUpdateInfo {
    /// State root of the execution payload
    pub state_root: H256,
    /// Branch indicating the state root in the tree corresponding to the execution payload
    pub state_root_branch: Vec<H256>,
    /// Block number of the execution payload
    pub block_number: U64,
    /// Branch indicating the block number in the tree corresponding to the execution payload
    pub block_number_branch: Vec<H256>,
}

impl ExecutionUpdate for ExecutionUpdateInfo {
    fn state_root(&self) -> H256 {
        self.state_root
    }

    fn state_root_branch(&self) -> Vec<H256> {
        self.state_root_branch.clone()
    }

    fn block_number(&self) -> U64 {
        self.block_number
    }

    fn block_number_branch(&self) -> Vec<H256> {
        self.block_number_branch.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrustedSyncCommittee<const SYNC_COMMITTEE_SIZE: usize> {
    /// height(i.e. execution's block number) of consensus state to trusted sync committee stored at
    pub height: Height,
    /// trusted sync committee
    pub sync_committee: SyncCommittee<SYNC_COMMITTEE_SIZE>,
    /// since the consensus state contains a current and next sync committee, this flag determines which one to refer to
    pub is_next: bool,
}

impl<const SYNC_COMMITTEE_SIZE: usize> TrustedSyncCommittee<SYNC_COMMITTEE_SIZE> {
    pub fn validate(&self) -> Result<(), Error> {
        if self.height.revision_number() != ETHEREUM_CLIENT_REVISION_NUMBER {
            return Err(Error::UnexpectedHeightRevisionNumber {
                expected: ETHEREUM_CLIENT_REVISION_NUMBER,
                got: self.height.revision_number(),
            });
        }
        self.sync_committee.validate()?;
        Ok(())
    }
}

impl<const SYNC_COMMITTEE_SIZE: usize> TryFrom<ProtoTrustedSyncCommittee>
    for TrustedSyncCommittee<SYNC_COMMITTEE_SIZE>
{
    type Error = Error;

    fn try_from(value: ProtoTrustedSyncCommittee) -> Result<Self, Error> {
        let trusted_height = value
            .trusted_height
            .as_ref()
            .ok_or(Error::proto_missing("trusted_height"))?;
        Ok(TrustedSyncCommittee {
            height: Height::new(
                trusted_height.revision_number,
                trusted_height.revision_height,
            )?,
            sync_committee: SyncCommittee {
                pubkeys: Vector::<PublicKey, SYNC_COMMITTEE_SIZE>::from_iter(
                    value
                        .sync_committee
                        .as_ref()
                        .ok_or(Error::proto_missing("sync_committee"))?
                        .pubkeys
                        .clone()
                        .into_iter()
                        .map(|pk| pk.try_into())
                        .collect::<Result<Vec<PublicKey>, _>>()?,
                ),
                aggregate_pubkey: PublicKey::try_from(
                    value
                        .sync_committee
                        .ok_or(Error::proto_missing("sync_committee"))?
                        .aggregate_pubkey,
                )?,
            },
            is_next: value.is_next,
        })
    }
}

impl<const SYNC_COMMITTEE_SIZE: usize> From<TrustedSyncCommittee<SYNC_COMMITTEE_SIZE>>
    for ProtoTrustedSyncCommittee
{
    fn from(value: TrustedSyncCommittee<SYNC_COMMITTEE_SIZE>) -> Self {
        Self {
            trusted_height: Some(ProtoHeight {
                revision_number: value.height.revision_number(),
                revision_height: value.height.revision_height(),
            }),
            sync_committee: Some(ProtoSyncCommittee {
                pubkeys: value
                    .sync_committee
                    .pubkeys
                    .iter()
                    .map(|pk| pk.to_vec())
                    .collect(),
                aggregate_pubkey: value.sync_committee.aggregate_pubkey.to_vec(),
            }),
            is_next: value.is_next,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountUpdateInfo {
    pub account_proof: Vec<Vec<u8>>,
    pub account_storage_root: H256,
}

impl From<AccountUpdateInfo> for ProtoAccountUpdate {
    fn from(value: AccountUpdateInfo) -> Self {
        Self {
            account_proof: encode_account_proof(value.account_proof),
            account_storage_root: value.account_storage_root.as_bytes().to_vec(),
        }
    }
}

impl TryFrom<ProtoAccountUpdate> for AccountUpdateInfo {
    type Error = Error;
    fn try_from(value: ProtoAccountUpdate) -> Result<Self, Self::Error> {
        Ok(Self {
            account_proof: decode_eip1184_rlp_proof(value.account_proof)?,
            account_storage_root: H256::from_slice(&value.account_storage_root),
        })
    }
}

fn encode_account_proof(bz: Vec<Vec<u8>>) -> Vec<u8> {
    let proof: Vec<Vec<u8>> = bz.into_iter().map(|b| b.to_vec()).collect();
    let mut stream = rlp::RlpStream::new();
    stream.begin_list(proof.len());
    for p in proof.iter() {
        stream.append_raw(p, 1);
    }
    stream.out().freeze().into()
}

pub(crate) fn convert_proto_to_header(
    header: &ProtoBeaconBlockHeader,
) -> Result<BeaconBlockHeader, Error> {
    Ok(BeaconBlockHeader {
        slot: header.slot.into(),
        proposer_index: header.proposer_index.into(),
        parent_root: H256::from_slice(&header.parent_root),
        state_root: H256::from_slice(&header.state_root),
        body_root: H256::from_slice(&header.body_root),
    })
}

pub(crate) fn convert_header_to_proto(header: &BeaconBlockHeader) -> ProtoBeaconBlockHeader {
    ProtoBeaconBlockHeader {
        slot: header.slot.into(),
        proposer_index: header.proposer_index.into(),
        parent_root: header.parent_root.as_bytes().to_vec(),
        state_root: header.state_root.as_bytes().to_vec(),
        body_root: header.body_root.as_bytes().to_vec(),
    }
}

pub(crate) fn convert_proto_to_execution_update(
    execution_update: ProtoExecutionUpdate,
) -> ExecutionUpdateInfo {
    ExecutionUpdateInfo {
        state_root: H256::from_slice(&execution_update.state_root),
        state_root_branch: execution_update
            .state_root_branch
            .into_iter()
            .map(|n| H256::from_slice(&n))
            .collect(),
        block_number: execution_update.block_number.into(),
        block_number_branch: execution_update
            .block_number_branch
            .into_iter()
            .map(|n| H256::from_slice(&n))
            .collect(),
    }
}

pub(crate) fn convert_execution_update_to_proto(
    execution_update: ExecutionUpdateInfo,
) -> ProtoExecutionUpdate {
    ProtoExecutionUpdate {
        state_root: execution_update.state_root.as_bytes().into(),
        state_root_branch: execution_update
            .state_root_branch
            .into_iter()
            .map(|n| n.as_bytes().to_vec())
            .collect(),
        block_number: execution_update.block_number.into(),
        block_number_branch: execution_update
            .block_number_branch
            .into_iter()
            .map(|n| n.as_bytes().to_vec())
            .collect(),
    }
}

/// CONTRACT: `SYNC_COMMITTEE_SIZE` must be greater than 0
pub(crate) fn convert_sync_aggregate_to_proto<const SYNC_COMMITTEE_SIZE: usize>(
    sync_aggregate: SyncAggregate<SYNC_COMMITTEE_SIZE>,
) -> ProtoSyncAggregate {
    let sync_committee_bits = ssz_rs::serialize(&sync_aggregate.sync_committee_bits)
        .expect("failed to serialize sync_committee_bits: this should never happen unless `SYNC_COMMITTEE_SIZE` is 0");
    ProtoSyncAggregate {
        sync_committee_bits,
        sync_committee_signature: sync_aggregate.sync_committee_signature.0.to_vec(),
    }
}

pub(crate) fn convert_proto_sync_aggregate<const SYNC_COMMITTEE_SIZE: usize>(
    sync_aggregate: ProtoSyncAggregate,
) -> Result<SyncAggregate<SYNC_COMMITTEE_SIZE>, Error> {
    Ok(SyncAggregate {
        sync_committee_bits: Bitvector::<SYNC_COMMITTEE_SIZE>::deserialize(
            sync_aggregate.sync_committee_bits.as_slice(),
        )
        .map_err(|e| Error::DeserializeSyncCommitteeBitsError {
            parent: e,
            sync_committee_size: SYNC_COMMITTEE_SIZE,
            sync_committee_bits: sync_aggregate.sync_committee_bits,
        })?,
        sync_committee_signature: Signature::try_from(sync_aggregate.sync_committee_signature)?,
    })
}

pub(crate) fn convert_consensus_update_to_proto<const SYNC_COMMITTEE_SIZE: usize>(
    consensus_update: ConsensusUpdateInfo<SYNC_COMMITTEE_SIZE>,
) -> ProtoConsensusUpdate {
    let finalized_beacon_header_branch = consensus_update.finalized_beacon_header_branch();
    let sync_aggregate = consensus_update.sync_aggregate.clone();

    ProtoConsensusUpdate {
        attested_header: Some(convert_header_to_proto(&consensus_update.attested_header)),
        next_sync_committee: consensus_update.next_sync_committee.clone().map(|c| {
            ProtoSyncCommittee {
                pubkeys: c.0.pubkeys.iter().map(|pk| pk.to_vec()).collect(),
                aggregate_pubkey: c.0.aggregate_pubkey.to_vec(),
            }
        }),
        next_sync_committee_branch: consensus_update
            .next_sync_committee
            .map_or(Vec::new(), |(_, branch)| {
                branch.into_iter().map(|n| n.as_bytes().to_vec()).collect()
            }),
        finalized_header: Some(convert_header_to_proto(
            &consensus_update.finalized_header.0,
        )),
        finalized_header_branch: finalized_beacon_header_branch
            .into_iter()
            .map(|n| n.as_bytes().to_vec())
            .collect(),
        finalized_execution_root: consensus_update.finalized_execution_root.as_bytes().into(),
        finalized_execution_branch: consensus_update
            .finalized_execution_branch
            .into_iter()
            .map(|n| n.as_bytes().to_vec())
            .collect(),
        sync_aggregate: Some(convert_sync_aggregate_to_proto(sync_aggregate)),
        signature_slot: consensus_update.signature_slot.into(),
    }
}

pub(crate) fn convert_proto_to_consensus_update<const SYNC_COMMITTEE_SIZE: usize>(
    consensus_update: ProtoConsensusUpdate,
) -> Result<ConsensusUpdateInfo<SYNC_COMMITTEE_SIZE>, Error> {
    let attested_header = convert_proto_to_header(
        consensus_update
            .attested_header
            .as_ref()
            .ok_or(Error::proto_missing("attested_header"))?,
    )?;
    let finalized_header = convert_proto_to_header(
        consensus_update
            .finalized_header
            .as_ref()
            .ok_or(Error::proto_missing("finalized_header"))?,
    )?;

    let finalized_execution_branch = consensus_update
        .finalized_execution_branch
        .into_iter()
        .map(|b| H256::from_slice(&b))
        .collect::<Vec<H256>>();
    let consensus_update = ConsensusUpdateInfo {
        attested_header,
        next_sync_committee: if consensus_update.next_sync_committee.is_none()
            || consensus_update
                .next_sync_committee
                .as_ref()
                .ok_or(Error::proto_missing("next_sync_committee"))?
                .pubkeys
                .is_empty()
            || consensus_update.next_sync_committee_branch.is_empty()
        {
            None
        } else {
            Some((
                SyncCommittee {
                    pubkeys: Vector::<PublicKey, SYNC_COMMITTEE_SIZE>::from_iter(
                        consensus_update
                            .next_sync_committee
                            .clone()
                            .ok_or(Error::proto_missing("next_sync_committee"))?
                            .pubkeys
                            .into_iter()
                            .map(|pk| pk.try_into())
                            .collect::<Result<Vec<PublicKey>, _>>()?,
                    ),
                    aggregate_pubkey: PublicKey::try_from(
                        consensus_update
                            .next_sync_committee
                            .ok_or(Error::proto_missing("next_sync_committee"))?
                            .aggregate_pubkey,
                    )?,
                },
                decode_branch(consensus_update.next_sync_committee_branch),
            ))
        },
        finalized_header: (
            finalized_header,
            decode_branch(consensus_update.finalized_header_branch),
        ),
        sync_aggregate: convert_proto_sync_aggregate(
            consensus_update
                .sync_aggregate
                .ok_or(Error::proto_missing("sync_aggregate"))?,
        )?,
        signature_slot: consensus_update.signature_slot.into(),
        finalized_execution_root: H256::from_slice(&consensus_update.finalized_execution_root),
        finalized_execution_branch,
    };
    Ok(consensus_update)
}

pub(crate) fn decode_branch(bz: Vec<Vec<u8>>) -> Vec<H256> {
    bz.into_iter().map(|b| H256::from_slice(&b)).collect()
}
