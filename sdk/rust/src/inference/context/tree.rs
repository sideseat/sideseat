use std::collections::HashMap;

use super::error::CmError;
use super::types::{BranchId, BranchMeta, ConversationId, Node, NodeHeader, NodeId, UserId, now_micros};

// ---------------------------------------------------------------------------
// BranchDiff
// ---------------------------------------------------------------------------

/// Result of a two-branch diff produced by [`ConversationTree::diff`].
#[derive(Debug, Clone)]
pub struct BranchDiff {
    /// Most recent node present on both branches.
    pub common_ancestor: Option<NodeId>,
    /// Nodes on the shared path from root to the common ancestor.
    pub shared: Vec<NodeId>,
    /// Nodes exclusive to branch A (not reachable from B).
    pub only_in_a: Vec<NodeId>,
    /// Nodes exclusive to branch B (not reachable from A).
    pub only_in_b: Vec<NodeId>,
}

// ---------------------------------------------------------------------------
// ConversationTree
// ---------------------------------------------------------------------------

/// In-memory index of all nodes and branches for a single conversation.
///
/// Tracks the tip (most recent node) of each branch, the active branch,
/// per-user cursors, and per-branch sequence counters. All mutations
/// (`register`, `fork`, `rewind`, `checkout`) keep the index consistent
/// with respect to the append-only node tree.
#[derive(Clone)]
pub struct ConversationTree {
    conversation_id: ConversationId,
    headers: HashMap<NodeId, NodeHeader>,
    branches: HashMap<BranchId, BranchMeta>,
    tips: HashMap<BranchId, NodeId>,
    active_branch: BranchId,
    cursors: HashMap<UserId, NodeId>,
    next_sequence: HashMap<BranchId, u64>,
}

impl ConversationTree {
    pub fn new(conversation_id: ConversationId) -> Self {
        let main_branch = BranchId::new();
        let mut branches = HashMap::new();
        branches.insert(
            main_branch.clone(),
            BranchMeta {
                id: main_branch.clone(),
                conversation_id: conversation_id.clone(),
                parent_id: None,
                fork_node_id: None,
                crdt_seq_watermark: 0,
                name: "main".into(),
                created_at: now_micros(),
            },
        );
        let mut next_sequence = HashMap::new();
        next_sequence.insert(main_branch.clone(), 0);

        Self {
            conversation_id,
            headers: HashMap::new(),
            branches,
            tips: HashMap::new(),
            active_branch: main_branch,
            cursors: HashMap::new(),
            next_sequence,
        }
    }

    /// Index a newly appended node and advance the branch tip.
    pub fn register(&mut self, node: &Node) -> Result<NodeHeader, CmError> {
        let header = NodeHeader::from(node);
        self.headers.insert(node.id.clone(), header.clone());

        let current_tip_seq = self
            .tips
            .get(&node.branch_id)
            .and_then(|tip_id| self.headers.get(tip_id))
            .map(|h| h.sequence)
            .unwrap_or(0);

        if node.sequence >= current_tip_seq {
            self.tips.insert(node.branch_id.clone(), node.id.clone());
        }

        Ok(header)
    }

    /// Registers a node header without requiring the full node.
    /// Used by `load()` to reconstruct the tree from persisted headers.
    pub fn register_header(&mut self, header: NodeHeader) -> Result<(), CmError> {
        let current_tip_seq = self
            .tips
            .get(&header.branch_id)
            .and_then(|tip_id| self.headers.get(tip_id))
            .map(|h| h.sequence)
            .unwrap_or(0);

        if header.sequence >= current_tip_seq {
            self.tips.insert(header.branch_id.clone(), header.id.clone());
        }

        // Advance next_sequence so new nodes appended after load() get a unique,
        // monotonically increasing sequence number on this branch.
        let next = self.next_sequence.entry(header.branch_id.clone()).or_insert(0);
        if header.sequence + 1 > *next {
            *next = header.sequence + 1;
        }

        self.headers.insert(header.id.clone(), header);
        Ok(())
    }

    /// Allocate and return the next sequence number for `branch_id`.
    pub fn next_seq(&mut self, branch_id: &BranchId) -> u64 {
        let seq = self.next_sequence.entry(branch_id.clone()).or_insert(0);
        let current = *seq;
        *seq = current + 1;
        current
    }

    // -----------------------------------------------------------------------
    // Git-style ops
    // -----------------------------------------------------------------------

    /// Create a new branch forking from `from_node_id`. Returns the new branch ID.
    pub fn fork(
        &mut self,
        from_node_id: &NodeId,
        name: String,
    ) -> Result<BranchId, CmError> {
        self.fork_with_id(from_node_id, BranchId::new(), name)
    }

    /// Like [`fork`] but uses a caller-supplied branch ID (useful for deterministic IDs in tests).
    pub fn fork_with_id(
        &mut self,
        from_node_id: &NodeId,
        new_branch: BranchId,
        name: String,
    ) -> Result<BranchId, CmError> {
        if !self.headers.contains_key(from_node_id) {
            return Err(CmError::NodeNotFound(from_node_id.clone()));
        }

        let header = &self.headers[from_node_id];
        let parent_branch_id = header.branch_id.clone();
        let crdt_watermark = header.crdt_seq_watermark.unwrap_or(0);
        self.branches.insert(
            new_branch.clone(),
            BranchMeta {
                id: new_branch.clone(),
                conversation_id: self.conversation_id.clone(),
                parent_id: Some(parent_branch_id),
                fork_node_id: Some(from_node_id.clone()),
                crdt_seq_watermark: crdt_watermark,
                name,
                created_at: now_micros(),
            },
        );

        self.tips
            .insert(new_branch.clone(), from_node_id.clone());
        self.next_sequence.insert(new_branch.clone(), header.sequence + 1);

        Ok(new_branch)
    }

    /// Moves the branch tip backward to `to_node_id`. Returns the IDs of nodes
    /// removed from the active path (they remain in the tree for future forking).
    pub fn rewind(&mut self, to_node_id: &NodeId) -> Result<Vec<NodeId>, CmError> {
        let header = self
            .headers
            .get(to_node_id)
            .ok_or_else(|| CmError::NodeNotFound(to_node_id.clone()))?;

        let branch_id = header.branch_id.clone();
        let target_seq = header.sequence;

        let current_path = self.linearize_ids(&branch_id)?;
        let pruned: Vec<NodeId> = current_path
            .into_iter()
            .filter(|id| {
                self.headers
                    .get(id)
                    .map(|h| h.sequence > target_seq)
                    .unwrap_or(false)
            })
            .collect();

        self.tips.insert(branch_id.clone(), to_node_id.clone());
        self.next_sequence
            .insert(branch_id, target_seq + 1);

        Ok(pruned)
    }

    /// Set `branch_id` as the active branch. Does not affect CRDT or VFS state.
    pub fn checkout(&mut self, branch_id: &BranchId) -> Result<(), CmError> {
        if !self.branches.contains_key(branch_id) {
            return Err(CmError::BranchNotFound(branch_id.clone()));
        }
        self.active_branch = branch_id.clone();
        Ok(())
    }

    /// Compute the symmetric difference between two branches.
    pub fn diff(
        &self,
        branch_a: &BranchId,
        branch_b: &BranchId,
    ) -> Result<BranchDiff, CmError> {
        let path_a = self.linearize_ids(branch_a)?;
        let path_b = self.linearize_ids(branch_b)?;

        let a_set: std::collections::HashSet<&NodeId> = path_a.iter().collect();
        let b_set: std::collections::HashSet<&NodeId> = path_b.iter().collect();

        let mut shared = Vec::new();
        let mut common_ancestor = None;

        for id in &path_a {
            if b_set.contains(id) {
                shared.push(id.clone());
                common_ancestor = Some(id.clone());
            }
        }

        let only_in_a: Vec<NodeId> = path_a
            .iter()
            .filter(|id| !b_set.contains(id))
            .cloned()
            .collect();
        let only_in_b: Vec<NodeId> = path_b
            .iter()
            .filter(|id| !a_set.contains(id))
            .cloned()
            .collect();

        Ok(BranchDiff {
            common_ancestor,
            shared,
            only_in_a,
            only_in_b,
        })
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Walk parent links from the branch tip to the root, returning headers in root-first order.
    pub fn linearize(&self, branch_id: &BranchId) -> Result<Vec<&NodeHeader>, CmError> {
        if !self.branches.contains_key(branch_id) {
            return Err(CmError::BranchNotFound(branch_id.clone()));
        }

        let tip_id = match self.tips.get(branch_id) {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        let mut path = Vec::new();
        let mut current = Some(tip_id.clone());
        let mut visited = std::collections::HashSet::new();

        while let Some(id) = current {
            if !visited.insert(id.clone()) {
                return Err(CmError::CycleDetected(format!(
                    "node {} in conversation tree",
                    id.as_str()
                )));
            }
            let header = match self.headers.get(&id) {
                Some(h) => h,
                None => break,
            };
            path.push(header);
            current = header.parent_id.clone();
        }

        path.reverse();
        Ok(path)
    }

    /// Like [`linearize`] but returns node IDs only.
    pub fn linearize_ids(&self, branch_id: &BranchId) -> Result<Vec<NodeId>, CmError> {
        Ok(self
            .linearize(branch_id)?
            .into_iter()
            .map(|h| h.id.clone())
            .collect())
    }

    /// All non-deleted children of `parent_id` (parallel response variants).
    pub fn variants(&self, parent_id: &NodeId) -> Vec<&NodeHeader> {
        self.headers
            .values()
            .filter(|h| h.parent_id.as_ref() == Some(parent_id) && !h.deleted)
            .collect()
    }

    /// Return the linearized sub-branch rooted at an `AgentSpawn` node,
    /// or just the spawn node itself if no sub-branch has been created yet.
    pub fn agent_subtree(
        &self,
        agent_spawn_id: &NodeId,
    ) -> Result<Vec<&NodeHeader>, CmError> {
        let spawn_header = self
            .headers
            .get(agent_spawn_id)
            .ok_or_else(|| CmError::NodeNotFound(agent_spawn_id.clone()))?;

        let sub_branch = self
            .branches
            .values()
            .find(|b| b.fork_node_id.as_ref() == Some(agent_spawn_id));

        match sub_branch {
            Some(branch) => self.linearize(&branch.id),
            None => Ok(vec![spawn_header]),
        }
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    /// Currently checked-out branch.
    pub fn active_branch(&self) -> &BranchId {
        &self.active_branch
    }

    /// All registered branches (including deleted ones not yet pruned).
    pub fn branches(&self) -> &HashMap<BranchId, BranchMeta> {
        &self.branches
    }

    /// Most recently appended node on `branch_id`, or `None` if the branch has no nodes.
    pub fn branch_tip(&self, branch_id: &BranchId) -> Option<&NodeId> {
        self.tips.get(branch_id)
    }

    /// Returns a reference to the header or `CmError::NodeNotFound`.
    pub fn get_header(&self, id: &NodeId) -> Result<&NodeHeader, CmError> {
        self.headers
            .get(id)
            .ok_or_else(|| CmError::NodeNotFound(id.clone()))
    }

    /// Returns the branch that owns `node_id`, or `None` if not registered.
    pub fn branch_of(&self, node_id: &NodeId) -> Option<BranchId> {
        self.headers.get(node_id).map(|h| h.branch_id.clone())
    }

    /// Cursor (last-viewed node) for `user_id`, or `None` if not set.
    pub fn cursor(&self, user_id: &UserId) -> Option<&NodeId> {
        self.cursors.get(user_id)
    }

    pub fn set_cursor(&mut self, user_id: UserId, node_id: NodeId) {
        self.cursors.insert(user_id, node_id);
    }

    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// Adds a branch directly (used during load to restore persisted branches).
    pub fn add_branch(&mut self, branch: BranchMeta) {
        // If the fork node header is already registered (live fork path), initialise
        // the branch tip and next_sequence immediately so that sub-agents can call
        // `spawn_agent` on a freshly forked child without first adding a node.
        if let Some(fork_node) = &branch.fork_node_id
            && let Some(h) = self.headers.get(fork_node)
        {
            self.tips.entry(branch.id.clone()).or_insert_with(|| fork_node.clone());
            self.next_sequence.entry(branch.id.clone()).or_insert(h.sequence + 1);
        } else {
            self.next_sequence.entry(branch.id.clone()).or_insert(0);
        }
        self.branches.insert(branch.id.clone(), branch);
    }

    /// For fork branches loaded from the backend before their fork node header was
    /// registered, set the branch tip and next_sequence retroactively.
    ///
    /// Call once at the end of `ContextManager::load()` after all node headers have
    /// been registered.
    pub fn initialize_fork_branch_tips(&mut self) {
        let branch_ids: Vec<BranchId> = self.branches.keys().cloned().collect();
        for branch_id in branch_ids {
            if self.tips.contains_key(&branch_id) {
                continue;
            }
            let fork_node = self.branches[&branch_id].fork_node_id.clone();
            if let Some(fork_node) = fork_node
                && let Some(h) = self.headers.get(&fork_node)
            {
                let seq = h.sequence;
                self.tips.insert(branch_id.clone(), fork_node);
                self.next_sequence.entry(branch_id).and_modify(|s| {
                    if *s == 0 {
                        *s = seq + 1;
                    }
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContentBlock;
    use super::super::types::{NodeContent, Node};
    use std::collections::HashMap;

    fn make_node(
        conv_id: &ConversationId,
        branch_id: &BranchId,
        parent_id: Option<NodeId>,
        seq: u64,
    ) -> Node {
        Node {
            id: NodeId::new(),
            conversation_id: conv_id.clone(),
            branch_id: branch_id.clone(),
            parent_id,
            sequence: seq,
            created_at: now_micros(),
            created_by: None,
            model: None,
            provider: None,
            content: NodeContent::UserMessage {
                content: vec![ContentBlock::text("test")],
                name: None,
            },
            usage: None,
            version: 0,
            is_final: true,
            streaming: None,
            deleted: false,
            agent_id: None,
            correlation_id: None,
            reply_to: None,
            eval_scores: Vec::new(),
            metadata: HashMap::new(),
            crdt_seq_watermark: None,
        }
    }

    #[test]
    fn register_and_linearize() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();

        let n0 = make_node(&conv_id, &branch, None, 0);
        let n0_id = n0.id.clone();
        tree.register(&n0).unwrap();

        let n1 = make_node(&conv_id, &branch, Some(n0_id.clone()), 1);
        let n1_id = n1.id.clone();
        tree.register(&n1).unwrap();

        let n2 = make_node(&conv_id, &branch, Some(n1_id.clone()), 2);
        let n2_id = n2.id.clone();
        tree.register(&n2).unwrap();

        let path = tree.linearize(&branch).unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].id, n0_id);
        assert_eq!(path[1].id, n1_id);
        assert_eq!(path[2].id, n2_id);
    }

    #[test]
    fn fork_and_checkout() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let main = tree.active_branch().clone();

        let n0 = make_node(&conv_id, &main, None, 0);
        let n0_id = n0.id.clone();
        tree.register(&n0).unwrap();

        let n1 = make_node(&conv_id, &main, Some(n0_id.clone()), 1);
        tree.register(&n1).unwrap();

        let fork_branch = tree.fork(&n0_id, "alt".into()).unwrap();

        let n2 = make_node(&conv_id, &fork_branch, Some(n0_id.clone()), 1);
        tree.register(&n2).unwrap();

        tree.checkout(&fork_branch).unwrap();
        assert_eq!(tree.active_branch(), &fork_branch);

        let fork_path = tree.linearize(&fork_branch).unwrap();
        assert_eq!(fork_path.len(), 2); // n0 + n2

        let main_path = tree.linearize(&main).unwrap();
        assert_eq!(main_path.len(), 2); // n0 + n1
    }

    #[test]
    fn rewind_returns_pruned() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();

        let n0 = make_node(&conv_id, &branch, None, 0);
        let n0_id = n0.id.clone();
        tree.register(&n0).unwrap();

        let n1 = make_node(&conv_id, &branch, Some(n0_id.clone()), 1);
        let n1_id = n1.id.clone();
        tree.register(&n1).unwrap();

        let n2 = make_node(&conv_id, &branch, Some(n1_id.clone()), 2);
        let n2_id = n2.id.clone();
        tree.register(&n2).unwrap();

        let pruned = tree.rewind(&n0_id).unwrap();
        assert_eq!(pruned.len(), 2);
        assert!(pruned.contains(&n1_id));
        assert!(pruned.contains(&n2_id));

        assert_eq!(tree.branch_tip(&branch), Some(&n0_id));
    }

    #[test]
    fn diff_branches() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let main = tree.active_branch().clone();

        let n0 = make_node(&conv_id, &main, None, 0);
        let n0_id = n0.id.clone();
        tree.register(&n0).unwrap();

        let n1 = make_node(&conv_id, &main, Some(n0_id.clone()), 1);
        tree.register(&n1).unwrap();

        let fork_branch = tree.fork(&n0_id, "alt".into()).unwrap();

        let n2 = make_node(&conv_id, &fork_branch, Some(n0_id.clone()), 1);
        tree.register(&n2).unwrap();

        let diff = tree.diff(&main, &fork_branch).unwrap();
        assert_eq!(diff.shared.len(), 1); // n0
        assert_eq!(diff.only_in_a.len(), 1);
        assert_eq!(diff.only_in_b.len(), 1);
    }

    #[test]
    fn variants_with_same_parent() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();

        let n0 = make_node(&conv_id, &branch, None, 0);
        let n0_id = n0.id.clone();
        tree.register(&n0).unwrap();

        let n1 = make_node(&conv_id, &branch, Some(n0_id.clone()), 1);
        tree.register(&n1).unwrap();

        let n2 = make_node(&conv_id, &branch, Some(n0_id.clone()), 2);
        tree.register(&n2).unwrap();

        let variants = tree.variants(&n0_id);
        assert_eq!(variants.len(), 2);
    }

    #[test]
    fn next_seq_monotonic() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id);
        let branch = tree.active_branch().clone();

        assert_eq!(tree.next_seq(&branch), 0);
        assert_eq!(tree.next_seq(&branch), 1);
        assert_eq!(tree.next_seq(&branch), 2);
    }

    #[test]
    fn agent_subtree_isolation() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let main = tree.active_branch().clone();

        let n0 = make_node(&conv_id, &main, None, 0);
        let n0_id = n0.id.clone();
        tree.register(&n0).unwrap();

        let spawn = make_node(&conv_id, &main, Some(n0_id.clone()), 1);
        let spawn_id = spawn.id.clone();
        tree.register(&spawn).unwrap();

        let agent_branch = tree.fork(&spawn_id, "agent/search".into()).unwrap();

        let a1 = make_node(&conv_id, &agent_branch, Some(spawn_id.clone()), 2);
        tree.register(&a1).unwrap();

        let a2 = make_node(&conv_id, &agent_branch, Some(a1.id.clone()), 3);
        tree.register(&a2).unwrap();

        let subtree = tree.agent_subtree(&spawn_id).unwrap();
        // Linearize follows parent chain from tip: a2 → a1 → spawn → n0
        assert_eq!(subtree.len(), 4);
    }

    #[test]
    fn register_header_roundtrip() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();

        // Build a node and its header directly
        let node = make_node(&conv_id, &branch, None, 0);
        let header = NodeHeader::from(&node);
        let node_id = header.id.clone();

        tree.register_header(header).unwrap();

        let retrieved = tree.get_header(&node_id).unwrap();
        assert_eq!(retrieved.id, node_id);
        assert_eq!(retrieved.branch_id, branch);
        assert_eq!(retrieved.sequence, 0);
        assert_eq!(retrieved.crdt_seq_watermark, None);
    }

    #[test]
    fn register_header_with_watermark() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();

        let mut node = make_node(&conv_id, &branch, None, 0);
        node.crdt_seq_watermark = Some(42);
        let header = NodeHeader::from(&node);
        let node_id = header.id.clone();

        tree.register_header(header).unwrap();

        let h = tree.get_header(&node_id).unwrap();
        assert_eq!(h.crdt_seq_watermark, Some(42));
    }

    #[test]
    fn branch_of_returns_correct_branch() {
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let main = tree.active_branch().clone();

        let n0 = make_node(&conv_id, &main, None, 0);
        let n0_id = n0.id.clone();
        tree.register(&n0).unwrap();

        let fork_branch = tree.fork(&n0_id, "fork".into()).unwrap();
        let n1 = make_node(&conv_id, &fork_branch, Some(n0_id.clone()), 1);
        let n1_id = n1.id.clone();
        tree.register(&n1).unwrap();

        assert_eq!(tree.branch_of(&n0_id), Some(main));
        assert_eq!(tree.branch_of(&n1_id), Some(fork_branch));
        assert_eq!(tree.branch_of(&NodeId::new()), None);
    }

    #[test]
    fn get_header_returns_error_when_missing() {
        let conv_id = ConversationId::new();
        let tree = ConversationTree::new(conv_id);
        let missing = NodeId::new();
        assert!(tree.get_header(&missing).is_err());
    }

    #[test]
    fn register_header_updates_next_sequence() {
        // After load(), new nodes must not collide with loaded sequences.
        let conv_id = ConversationId::new();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();

        // Simulate loading headers that already occupy seqs 0..=4.
        for seq in 0u64..=4 {
            let node = make_node(&conv_id, &branch, None, seq);
            let header = NodeHeader::from(&node);
            tree.register_header(header).unwrap();
        }

        // next_seq must yield 5, not 0.
        assert_eq!(
            tree.next_seq(&branch),
            5,
            "next_sequence must be max_loaded_seq + 1 after register_header"
        );
    }
}
