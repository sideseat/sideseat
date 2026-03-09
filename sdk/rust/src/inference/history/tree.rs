use std::collections::HashMap;

use super::error::HistoryError;
use super::types::{
    BranchId, BranchMeta, ConversationId, Node, NodeHeader, NodeId, UserId, now_micros,
};

// ---------------------------------------------------------------------------
// BranchDiff
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BranchDiff {
    pub common_ancestor: Option<NodeId>,
    pub shared: Vec<NodeId>,
    pub only_in_a: Vec<NodeId>,
    pub only_in_b: Vec<NodeId>,
}

// ---------------------------------------------------------------------------
// ConversationTree
// ---------------------------------------------------------------------------

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
                parent_branch_id: None,
                fork_node_id: None,
                created_at: now_micros(),
                created_by: None,
                name: Some("main".into()),
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

    pub fn register(&mut self, node: &Node) -> Result<NodeHeader, HistoryError> {
        let header = NodeHeader::from(node);
        self.headers.insert(node.id.clone(), header.clone());

        let current_tip_seq = self
            .tips
            .get(&node.branch_id)
            .and_then(|tip_id| self.headers.get(tip_id))
            .map(|h| h.sequence)
            .unwrap_or(0);

        if node.sequence >= current_tip_seq || !self.tips.contains_key(&node.branch_id) {
            self.tips.insert(node.branch_id.clone(), node.id.clone());
        }

        Ok(header)
    }

    pub fn next_seq(&mut self, branch_id: &BranchId) -> u64 {
        let seq = self.next_sequence.entry(branch_id.clone()).or_insert(0);
        let current = *seq;
        *seq = current + 1;
        current
    }

    // -----------------------------------------------------------------------
    // Git-style ops
    // -----------------------------------------------------------------------

    pub fn fork(
        &mut self,
        from_node_id: &NodeId,
        name: Option<String>,
    ) -> Result<BranchId, HistoryError> {
        self.fork_with_id(from_node_id, BranchId::new(), name)
    }

    pub fn fork_with_id(
        &mut self,
        from_node_id: &NodeId,
        new_branch: BranchId,
        name: Option<String>,
    ) -> Result<BranchId, HistoryError> {
        if !self.headers.contains_key(from_node_id) {
            return Err(HistoryError::NodeNotFound(from_node_id.clone()));
        }

        let header = &self.headers[from_node_id];
        let parent_branch_id = header.branch_id.clone();
        self.branches.insert(
            new_branch.clone(),
            BranchMeta {
                id: new_branch.clone(),
                conversation_id: self.conversation_id.clone(),
                parent_branch_id: Some(parent_branch_id),
                fork_node_id: Some(from_node_id.clone()),
                created_at: now_micros(),
                created_by: None,
                name,
            },
        );

        self.tips
            .insert(new_branch.clone(), from_node_id.clone());
        self.next_sequence.insert(new_branch.clone(), header.sequence + 1);

        Ok(new_branch)
    }

    /// Moves the branch tip backward to `to_node_id`. Returns the IDs of nodes
    /// removed from the active path (they remain in the tree for future forking).
    pub fn rewind(&mut self, to_node_id: &NodeId) -> Result<Vec<NodeId>, HistoryError> {
        let header = self
            .headers
            .get(to_node_id)
            .ok_or_else(|| HistoryError::NodeNotFound(to_node_id.clone()))?;

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

    pub fn checkout(&mut self, branch_id: &BranchId) -> Result<(), HistoryError> {
        if !self.branches.contains_key(branch_id) {
            return Err(HistoryError::BranchNotFound(branch_id.clone()));
        }
        self.active_branch = branch_id.clone();
        Ok(())
    }

    pub fn diff(
        &self,
        branch_a: &BranchId,
        branch_b: &BranchId,
    ) -> Result<BranchDiff, HistoryError> {
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

    pub fn linearize(&self, branch_id: &BranchId) -> Result<Vec<&NodeHeader>, HistoryError> {
        if !self.branches.contains_key(branch_id) {
            return Err(HistoryError::BranchNotFound(branch_id.clone()));
        }

        let tip_id = match self.tips.get(branch_id) {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        let mut path = Vec::new();
        let mut current = Some(tip_id.clone());

        while let Some(id) = current {
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

    pub fn linearize_ids(&self, branch_id: &BranchId) -> Result<Vec<NodeId>, HistoryError> {
        Ok(self
            .linearize(branch_id)?
            .into_iter()
            .map(|h| h.id.clone())
            .collect())
    }

    pub fn variants(&self, parent_id: &NodeId) -> Vec<&NodeHeader> {
        self.headers
            .values()
            .filter(|h| h.parent_id.as_ref() == Some(parent_id) && !h.deleted)
            .collect()
    }

    pub fn agent_subtree(
        &self,
        agent_spawn_id: &NodeId,
    ) -> Result<Vec<&NodeHeader>, HistoryError> {
        let spawn_header = self
            .headers
            .get(agent_spawn_id)
            .ok_or_else(|| HistoryError::NodeNotFound(agent_spawn_id.clone()))?;

        // Find branch forked from this spawn node
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

    pub fn active_branch(&self) -> &BranchId {
        &self.active_branch
    }

    pub fn branches(&self) -> &HashMap<BranchId, BranchMeta> {
        &self.branches
    }

    pub fn branch_tip(&self, branch_id: &BranchId) -> Option<&NodeId> {
        self.tips.get(branch_id)
    }

    pub fn get_header(&self, id: &NodeId) -> Option<&NodeHeader> {
        self.headers.get(id)
    }

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
        self.next_sequence
            .entry(branch.id.clone())
            .or_insert(0);
        self.branches.insert(branch.id.clone(), branch);
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
        let _n1_id = n1.id.clone();
        tree.register(&n1).unwrap();

        let fork_branch = tree.fork(&n0_id, Some("alt".into())).unwrap();

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

        let fork_branch = tree.fork(&n0_id, None).unwrap();

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

        // Two children with same parent (variant siblings)
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

        // Spawn node
        let spawn = make_node(&conv_id, &main, Some(n0_id.clone()), 1);
        let spawn_id = spawn.id.clone();
        tree.register(&spawn).unwrap();

        // Fork for agent
        let agent_branch = tree.fork(&spawn_id, Some("agent/search".into())).unwrap();

        let a1 = make_node(&conv_id, &agent_branch, Some(spawn_id.clone()), 2);
        tree.register(&a1).unwrap();

        let a2 = make_node(&conv_id, &agent_branch, Some(a1.id.clone()), 3);
        tree.register(&a2).unwrap();

        let subtree = tree.agent_subtree(&spawn_id).unwrap();
        // Linearize follows parent chain from tip: a2 → a1 → spawn → n0
        assert_eq!(subtree.len(), 4);
    }
}
