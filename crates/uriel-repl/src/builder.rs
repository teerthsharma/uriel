use uriel_core::{Command, ConstraintKind, NodeId, Props, RelId, WriteBatch};

pub struct BatchBuilder {
    epoch: u64,
    next_node: u64,
    next_rel: u64,
    commands: Vec<Command>,
}

impl BatchBuilder {
    pub(crate) fn new(epoch: u64, next_node: u64, next_rel: u64) -> BatchBuilder {
        BatchBuilder { epoch, next_node, next_rel, commands: Vec::new() }
    }

    pub(crate) fn finish(self) -> WriteBatch {
        WriteBatch { epoch: self.epoch, commands: self.commands }
    }

    pub fn create_node(&mut self, labels: &[&str], props: Props) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node += 1;
        let labels = labels.iter().map(|l| (*l).to_string()).collect();
        self.commands.push(Command::CreateNode { id, labels, props });
        id
    }

    pub fn create_rel(&mut self, src: NodeId, dst: NodeId, rel_type: &str, props: Props) -> RelId {
        let id = RelId(self.next_rel);
        self.next_rel += 1;
        self.commands.push(Command::CreateRel { id, src, dst, rel_type: rel_type.to_string(), props });
        id
    }

    pub fn set_node_props(&mut self, id: NodeId, props: Props) {
        self.commands.push(Command::SetNodeProps { id, props });
    }

    pub fn add_label(&mut self, id: NodeId, label: &str) {
        self.commands.push(Command::AddLabel { id, label: label.to_string() });
    }

    pub fn remove_label(&mut self, id: NodeId, label: &str) {
        self.commands.push(Command::RemoveLabel { id, label: label.to_string() });
    }

    pub fn delete_node(&mut self, id: NodeId) {
        self.commands.push(Command::DeleteNode { id });
    }

    pub fn set_rel_props(&mut self, id: RelId, props: Props) {
        self.commands.push(Command::SetRelProps { id, props });
    }

    pub fn delete_rel(&mut self, id: RelId) {
        self.commands.push(Command::DeleteRel { id });
    }

    pub fn create_index(&mut self, label: &str, key: &str) {
        self.commands.push(Command::CreateIndex { label: label.to_string(), key: key.to_string() });
    }

    pub fn drop_index(&mut self, label: &str, key: &str) {
        self.commands.push(Command::DropIndex { label: label.to_string(), key: key.to_string() });
    }

    pub fn create_constraint(&mut self, kind: ConstraintKind, label: &str, key: &str) {
        self.commands.push(Command::CreateConstraint {
            kind,
            label: label.to_string(),
            key: key.to_string(),
        });
    }

    pub fn drop_constraint(&mut self, kind: ConstraintKind, label: &str, key: &str) {
        self.commands.push(Command::DropConstraint {
            kind,
            label: label.to_string(),
            key: key.to_string(),
        });
    }
}
