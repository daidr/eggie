use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Default working directory for new terminals in this project. `None` means the user's home
    /// directory is used.
    pub root: Option<PathBuf>,
    /// Profile new terminals in this project launch with. `None` means the global default profile.
    /// A stale id (the profile was deleted) also resolves to the global default rather than failing.
    #[serde(default)]
    pub default_profile_id: Option<ProfileId>,
}

impl Project {
    pub fn from_root(root: PathBuf) -> Self {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Project")
            .to_owned();
        Self {
            id: Uuid::new_v4(),
            name,
            root: Some(root),
            default_profile_id: None,
        }
    }

    /// Create a project with only a name and no default working directory. New terminals will
    /// start in the user's home directory until a root is set.
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            root: None,
            default_profile_id: None,
        }
    }

    /// The working directory to use for new terminals: the configured root, or the user's home
    /// directory if none is set.
    pub fn effective_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| home_dir())
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
}
