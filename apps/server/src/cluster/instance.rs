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

/// What an instance does. Every instance does all three until K5.7 lets an
/// operator choose.
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
