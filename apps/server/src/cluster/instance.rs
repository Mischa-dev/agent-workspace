//! An instance of the server: its id, its roles and its row in
//! `server_instances` (K5 3.2).

use uuid::Uuid;

/// One running server process. A version 7 UUID, so ids sort by start.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct InstanceId(pub Uuid);

impl InstanceId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

/// What an instance does (K5.7). `api` answers the app's requests; `live`
/// holds browsers' sockets; `work` runs jobs, the log's consumers and loops,
/// and may hold the built-in host. Every instance does all three unless the
/// operator says otherwise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Roles {
    pub api: bool,
    pub live: bool,
    pub work: bool,
}

impl Roles {
    pub const ALL: Self = Self {
        api: true,
        live: true,
        work: true,
    };

    /// The names stored in `server_instances.roles`.
    pub fn names(self) -> Vec<String> {
        [("api", self.api), ("live", self.live), ("work", self.work)]
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(name, _)| name.to_owned())
            .collect()
    }
}

impl std::str::FromStr for Roles {
    type Err = String;

    /// A comma-separated list of `api`, `live` and `work`, at least one.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut roles = Self {
            api: false,
            live: false,
            work: false,
        };
        for name in value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            match name.to_ascii_lowercase().as_str() {
                "api" => roles.api = true,
                "live" => roles.live = true,
                "work" => roles.work = true,
                _ => return Err(format!("unknown role {name:?}; expected api, live or work")),
            }
        }
        if roles.names().is_empty() {
            return Err("name at least one of api, live and work".into());
        }
        Ok(roles)
    }
}

impl Default for Roles {
    fn default() -> Self {
        Self::ALL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instance_id_is_a_version_7_uuid() {
        assert_eq!(InstanceId::new().0.get_version_num(), 7);
        // The table's own check reads the version digit of the text form.
        assert_eq!(InstanceId::new().0.to_string().chars().nth(14), Some('7'));
    }

    #[test]
    fn roles_name_what_they_do() {
        assert_eq!(Roles::ALL.names(), ["api", "live", "work"]);
        let work = Roles {
            api: false,
            live: false,
            work: true,
        };
        assert_eq!(work.names(), ["work"]);
    }
}
