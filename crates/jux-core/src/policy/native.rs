#[derive(Clone, Debug, Eq, PartialEq)]
/// Policy for native host command execution.
///
/// Native commands are not treated as strongly sandboxed. This policy describes
/// whether they are available and which commands may be considered later.
pub struct NativeCommandPolicy {
    pub enabled: bool,
    pub allowed_commands: Vec<NativeCommandRule>,
}

impl NativeCommandPolicy {
    #[must_use]
    pub fn disabled() -> Self {
        let allowed_commands = Vec::new();
        Self {
            enabled: false,
            allowed_commands,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Allow-list entry for a native command.
pub struct NativeCommandRule {
    pub program: String,
}
