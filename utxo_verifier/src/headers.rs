use std::collections::{BTreeMap, HashMap};

use bitcoin::{
    BlockHash, CompactTarget, Network, Work,
    block::{Header, HeaderExt},
    constants::genesis_block,
    params::Params,
    pow::CompactTargetExt,
};

type Height = u32;

const LOCATOR_INDEX: &[Height] = &[1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024];

trait ZerolikeExt {
    fn zero() -> Self;
}

impl ZerolikeExt for Work {
    fn zero() -> Self {
        Self::from_be_bytes([0; 32])
    }
}

// Attributes of a height in the Bitcoin blockchain.
trait HeightExt: Clone + Copy + std::hash::Hash + PartialEq + Eq + PartialOrd + Ord {
    fn increment(&self) -> Self;

    fn from_u64_checked(height: u64) -> Option<Self>;

    fn is_adjustment_multiple(&self, params: impl AsRef<Params>) -> bool;

    fn last_epoch_start(&self, params: impl AsRef<Params>) -> Self;
}

impl HeightExt for u32 {
    fn increment(&self) -> Self {
        self + 1
    }

    fn is_adjustment_multiple(&self, params: impl AsRef<Params>) -> bool {
        *self as u64 % params.as_ref().difficulty_adjustment_interval() == 0
    }

    fn from_u64_checked(height: u64) -> Option<Self> {
        height.try_into().ok()
    }

    fn last_epoch_start(&self, params: impl AsRef<Params>) -> Self {
        let diff_adjustment_interval = params.as_ref().difficulty_adjustment_interval() as u32;
        let floor = self / diff_adjustment_interval;
        floor * diff_adjustment_interval
    }
}
/// A block header with associated height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedHeader {
    /// The height in the blockchain for this header.
    pub height: u32,
    /// The block header.
    pub header: Header,
}

impl std::cmp::PartialOrd for IndexedHeader {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for IndexedHeader {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.height.cmp(&other.height)
    }
}

impl IndexedHeader {
    pub(crate) fn new(height: u32, header: Header) -> Self {
        Self { height, header }
    }
}

#[derive(Debug, Clone)]
pub enum AcceptHeaderChanges {
    Accepted {
        connected_at: IndexedHeader,
    },
    Duplicate,
    ExtendedFork {
        connected_at: IndexedHeader,
    },
    Reorganization {
        accepted: Vec<IndexedHeader>,
        disconnected: Vec<IndexedHeader>,
    },
    Rejected(HeaderRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderRejection {
    InvalidPow {
        expected: CompactTarget,
        got: CompactTarget,
    },
    UnknownPrevHash(BlockHash),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tip {
    pub hash: BlockHash,
    pub height: Height,
    pub next_work_required: Option<CompactTarget>,
}

#[derive(Debug, Clone, Hash)]
pub(crate) struct BlockNode {
    pub height: Height,
    pub header: Header,
    pub acc_work: Work,
}

impl BlockNode {
    fn new(height: Height, header: Header, acc_work: Work) -> Self {
        Self {
            height,
            header,
            acc_work,
        }
    }
}

#[derive(Debug)]
pub struct BlockTree {
    canonical_hashes: BTreeMap<Height, BlockHash>,
    headers: HashMap<BlockHash, BlockNode>,
    active_tip: Tip,
    candidate_forks: Vec<Tip>,
    network: Network,
}

#[allow(unused)]
impl BlockTree {
    pub(crate) fn from_genesis(network: Network) -> Self {
        let genesis = genesis_block(network);
        let height = 0;
        let hash = genesis.block_hash();
        let tip = Tip {
            hash,
            height,
            next_work_required: Some(genesis.header().bits),
        };
        let mut headers = HashMap::with_capacity(20_000);
        let block_node = BlockNode::new(height, *genesis.header(), genesis.header().work());
        headers.insert(hash, block_node);
        let mut canonical_hashes = BTreeMap::new();
        canonical_hashes.insert(0, hash);
        Self {
            canonical_hashes,
            headers,
            active_tip: tip,
            candidate_forks: Vec::with_capacity(2),
            network,
        }
    }

    pub(crate) fn from_header(height: impl Into<Height>, header: Header, network: Network) -> Self {
        let height = height.into();
        let hash = header.block_hash();
        let tip = Tip {
            hash,
            height,
            next_work_required: Some(header.bits),
        };
        let mut headers = HashMap::with_capacity(20_000);
        let block_node = BlockNode::new(height, header, header.work());
        headers.insert(hash, block_node);
        Self {
            canonical_hashes: BTreeMap::new(),
            headers,
            active_tip: tip,
            candidate_forks: Vec::with_capacity(2),
            network,
        }
    }

    pub(crate) fn accept_header(&mut self, new_header: Header) -> AcceptHeaderChanges {
        let new_hash = new_header.block_hash();
        let prev_hash = new_header.prev_blockhash;

        if self.active_tip.hash.eq(&prev_hash) {
            let new_height = self.active_tip.height.increment();
            let params = self.network.params();
            let next_work = if !params.no_pow_retargeting
                && !params.allow_min_difficulty_blocks
                && new_height.is_adjustment_multiple(self.network)
            {
                self.compute_next_work_required(new_height)
            } else {
                self.active_tip.next_work_required
            };
            if let Some(work) = next_work {
                #[allow(clippy::collapsible_if)]
                if new_header.bits.ne(&work) {
                    return AcceptHeaderChanges::Rejected(HeaderRejection::InvalidPow {
                        expected: work,
                        got: new_header.bits,
                    });
                }
            }
            let new_tip = Tip {
                hash: new_hash,
                height: new_height,
                next_work_required: next_work,
            };
            let prev_work = self
                .headers
                .get(&prev_hash)
                .map(|block| block.acc_work)
                .unwrap_or(Work::zero());
            let new_work = prev_work + new_header.work();
            let new_block_node = BlockNode::new(new_height, new_header, new_work);
            self.headers.insert(new_hash, new_block_node);
            self.active_tip = new_tip;
            self.canonical_hashes.insert(new_height, new_hash);
            return AcceptHeaderChanges::Accepted {
                connected_at: IndexedHeader::new(new_height, new_header),
            };
        }

        if self.headers.contains_key(&new_hash) {
            return AcceptHeaderChanges::Duplicate;
        }

        if let Some(fork_index) = self
            .candidate_forks
            .iter()
            .position(|fork| fork.hash.eq(&prev_hash))
        {
            let fork = self.candidate_forks.swap_remove(fork_index);
            if let Some(node) = self.headers.get(&fork.hash) {
                let new_height = node.height.increment();
                let params = self.network.params();
                let next_work = if !params.no_pow_retargeting
                    && !params.allow_min_difficulty_blocks
                    && new_height.is_adjustment_multiple(self.network)
                {
                    self.compute_next_work_required(new_height)
                } else {
                    fork.next_work_required
                };
                if let Some(work) = next_work {
                    if new_header.bits.ne(&work) {
                        return AcceptHeaderChanges::Rejected(HeaderRejection::InvalidPow {
                            expected: work,
                            got: new_header.bits,
                        });
                    }
                }
                let acc_work = node.acc_work + new_header.work();
                let new_tip = Tip {
                    hash: new_hash,
                    height: new_height,
                    next_work_required: next_work,
                };
                let new_block_node = BlockNode::new(new_height, new_header, acc_work);
                self.headers.insert(new_hash, new_block_node);
                if acc_work
                    > self
                        .headers
                        .get(&self.active_tip.hash)
                        .map(|node| node.acc_work)
                        .unwrap_or(Work::zero())
                {
                    self.candidate_forks.push(self.active_tip);
                    self.active_tip = new_tip;
                    let (accepted, disconnected) = self.switch_to_fork(&new_tip);
                    return AcceptHeaderChanges::Reorganization {
                        accepted,
                        disconnected,
                    };
                } else {
                    self.candidate_forks.push(new_tip);
                    return AcceptHeaderChanges::ExtendedFork {
                        connected_at: IndexedHeader::new(new_height, new_header),
                    };
                }
            }
        }

        match self.headers.get(&prev_hash) {
            // A new fork was detected
            Some(node) => {
                let new_height = node.height.increment();
                let params = self.network.params();
                let next_work = if !params.no_pow_retargeting
                    && !params.allow_min_difficulty_blocks
                    && new_height.is_adjustment_multiple(self.network)
                {
                    self.compute_next_work_required(new_height)
                } else {
                    Some(node.header.bits)
                };
                if let Some(work) = next_work {
                    if new_header.bits.ne(&work) {
                        return AcceptHeaderChanges::Rejected(HeaderRejection::InvalidPow {
                            expected: work,
                            got: new_header.bits,
                        });
                    }
                }
                let acc_work = node.acc_work + new_header.work();
                let new_tip = Tip {
                    hash: new_hash,
                    height: new_height,
                    next_work_required: next_work,
                };
                self.candidate_forks.push(new_tip);
                let new_block_node = BlockNode::new(new_height, new_header, acc_work);
                self.headers.insert(new_hash, new_block_node);
                AcceptHeaderChanges::ExtendedFork {
                    connected_at: IndexedHeader::new(new_height, new_header),
                }
            }
            // This chain doesn't link to ours in any known way
            None => AcceptHeaderChanges::Rejected(HeaderRejection::UnknownPrevHash(prev_hash)),
        }
    }

    fn switch_to_fork(&mut self, new_best: &Tip) -> (Vec<IndexedHeader>, Vec<IndexedHeader>) {
        let mut curr_hash = new_best.hash;
        let mut connections = Vec::new();
        let mut disconnections = Vec::new();
        loop {
            match self.headers.get(&curr_hash) {
                Some(node) => {
                    let next = node.header.prev_blockhash;
                    match self.canonical_hashes.get_mut(&node.height) {
                        Some(canonical_hash) => {
                            let reorged_hash = *canonical_hash;
                            if reorged_hash.ne(&curr_hash) {
                                if let Some(reorged) = self.headers.get(&reorged_hash) {
                                    disconnections
                                        .push(IndexedHeader::new(reorged.height, reorged.header));
                                }
                                *canonical_hash = curr_hash;
                                connections.push(IndexedHeader::new(node.height, node.header));
                                curr_hash = next;
                            } else {
                                return (connections, disconnections);
                            }
                        }
                        None => {
                            self.canonical_hashes.insert(node.height, curr_hash);
                            connections.push(IndexedHeader::new(node.height, node.header));
                            curr_hash = next;
                        }
                    }
                }
                None => return (connections, disconnections),
            }
        }
    }

    fn compute_next_work_required(&self, new_height: Height) -> Option<CompactTarget> {
        // Do not audit the diffulty for `Testnet`. Auditing the difficulty properly for a testnet
        // will result in convoluted logic. This is a critical code block for mainnet and should be
        // as readable as possible
        if self.network.params().allow_min_difficulty_blocks {
            return None;
        }
        let adjustment_period =
            Height::from_u64_checked(self.network.params().difficulty_adjustment_interval())?;
        let epoch_start = new_height.checked_sub(adjustment_period)?;
        let epoch_end = new_height.checked_sub(1)?;
        let epoch_start_hash = self.canonical_hashes.get(&epoch_start)?;
        let epoch_end_hash = self.canonical_hashes.get(&epoch_end)?;
        let epoch_start_header = self.headers.get(epoch_start_hash).map(|node| node.header)?;
        let epoch_end_header = self.headers.get(epoch_end_hash).map(|node| node.header)?;
        let new_target = CompactTarget::from_header_difficulty_adjustment(
            epoch_start_header,
            epoch_end_header,
            self.network,
        );
        Some(new_target)
    }

    pub(crate) fn block_hash_at_height(&self, height: Height) -> Option<BlockHash> {
        if self.active_tip.height.eq(&height) {
            return Some(self.active_tip.hash);
        }
        self.canonical_hashes.get(&height).copied()
    }

    pub(crate) fn header_at_height(&self, height: Height) -> Option<Header> {
        let hash = self.canonical_hashes.get(&height)?;
        self.headers.get(hash).map(|node| node.header)
    }

    pub(crate) fn height_of_hash(&self, hash: BlockHash) -> Option<Height> {
        self.headers.get(&hash).map(|node| node.height)
    }

    pub(crate) fn header_at_hash(&self, hash: BlockHash) -> Option<Header> {
        self.headers.get(&hash).map(|node| node.header)
    }

    pub(crate) fn height(&self) -> Height {
        self.active_tip.height
    }

    pub(crate) fn contains(&self, hash: BlockHash) -> bool {
        self.headers.contains_key(&hash) || self.active_tip.hash.eq(&hash)
    }

    pub(crate) fn tip_hash(&self) -> BlockHash {
        self.active_tip.hash
    }

    pub(crate) fn locators(&self) -> Vec<BlockHash> {
        let mut locators = Vec::new();
        locators.push(self.active_tip.hash);
        for locator in LOCATOR_INDEX {
            let height = self.active_tip.height.checked_sub(*locator);
            match height {
                Some(height) => match self.block_hash_at_height(height) {
                    Some(hash) => locators.push(hash),
                    None => return locators.into_iter().rev().collect(),
                },
                None => return locators.into_iter().rev().collect(),
            }
        }
        locators.into_iter().rev().collect()
    }

    pub(crate) fn internal_chain_len(&self) -> usize {
        self.canonical_hashes.len()
    }

    pub(crate) fn iter_data(&self) -> BlockNodeIterator<'_> {
        BlockNodeIterator {
            block_tree: self,
            current: self.active_tip.hash,
        }
    }

    pub(crate) fn iter_headers(&self) -> BlockHeaderIterator<'_> {
        BlockHeaderIterator {
            block_tree: self,
            current: self.active_tip.hash,
        }
    }
}

pub(crate) struct BlockHeaderIterator<'a> {
    block_tree: &'a BlockTree,
    current: BlockHash,
}

impl Iterator for BlockHeaderIterator<'_> {
    type Item = IndexedHeader;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.block_tree.headers.get(&self.current)?;
        self.current = node.header.prev_blockhash;
        Some(IndexedHeader::new(node.height, node.header))
    }
}

pub(crate) struct BlockNodeIterator<'a> {
    block_tree: &'a BlockTree,
    current: BlockHash,
}

impl<'a> Iterator for BlockNodeIterator<'a> {
    type Item = &'a BlockNode;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.block_tree.headers.get(&self.current)?;
        self.current = node.header.prev_blockhash;
        Some(node)
    }
}
