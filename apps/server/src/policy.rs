use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorKind {
    Human,
    ExternalAgent,
    ExecutionWorker,
    Integration,
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
    Worker,
}

impl MembershipRole {
    pub const ALL: [Self; 5] = [
        Self::Owner,
        Self::Admin,
        Self::Member,
        Self::Viewer,
        Self::Worker,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    OrganizationProjectCreate,
    ProjectView,
    ProjectManage,
    InvitationManage,
    MembershipView,
    MembershipManage,
    NavigationRead,
    NavigationWrite,
    WorkerInvoke,
    RunInterrupt,
    ApprovalRespond,
    ChannelCreate,
    MessageSend,
    ThreadCreate,
    ConversationArchive,
    ReadStateUpdate,
    MentionSearch,
    WorkItemView,
    WorkItemCreate,
    WorkItemEdit,
    RepositoryPublish,
    PullRequestCreate,
    PullRequestRefresh,
}

impl Action {
    pub const ALL: [Self; 23] = [
        Self::OrganizationProjectCreate,
        Self::ProjectView,
        Self::ProjectManage,
        Self::InvitationManage,
        Self::MembershipView,
        Self::MembershipManage,
        Self::NavigationRead,
        Self::NavigationWrite,
        Self::WorkerInvoke,
        Self::RunInterrupt,
        Self::ApprovalRespond,
        Self::ChannelCreate,
        Self::MessageSend,
        Self::ThreadCreate,
        Self::ConversationArchive,
        Self::ReadStateUpdate,
        Self::MentionSearch,
        Self::WorkItemView,
        Self::WorkItemCreate,
        Self::WorkItemEdit,
        Self::RepositoryPublish,
        Self::PullRequestCreate,
        Self::PullRequestRefresh,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipScope {
    Organization(Uuid),
    Project(Uuid),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Membership {
    scope: MembershipScope,
    role: MembershipRole,
    enabled: bool,
}

impl Membership {
    pub const fn organization(organization_id: Uuid, role: MembershipRole, enabled: bool) -> Self {
        Self {
            scope: MembershipScope::Organization(organization_id),
            role,
            enabled,
        }
    }

    pub const fn project(project_id: Uuid, role: MembershipRole, enabled: bool) -> Self {
        Self {
            scope: MembershipScope::Project(project_id),
            role,
            enabled,
        }
    }

    pub const fn scope(self) -> MembershipScope {
        self.scope
    }

    pub const fn role(self) -> MembershipRole {
        self.role
    }

    pub const fn is_enabled(self) -> bool {
        self.enabled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectScope {
    pub project_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub repository_id: Option<Uuid>,
    pub approval_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
}

impl ProjectScope {
    pub const fn project(project_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: None,
            run_id: None,
            repository_id: None,
            approval_id: None,
            work_item_id: None,
        }
    }

    pub const fn thread(project_id: Uuid, thread_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: Some(thread_id),
            run_id: None,
            repository_id: None,
            approval_id: None,
            work_item_id: None,
        }
    }

    pub const fn run(project_id: Uuid, thread_id: Uuid, run_id: Uuid, repository_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: Some(thread_id),
            run_id: Some(run_id),
            repository_id: Some(repository_id),
            approval_id: None,
            work_item_id: None,
        }
    }

    pub const fn approval(
        project_id: Uuid,
        thread_id: Uuid,
        run_id: Uuid,
        repository_id: Uuid,
        approval_id: Uuid,
    ) -> Self {
        Self {
            project_id,
            thread_id: Some(thread_id),
            run_id: Some(run_id),
            repository_id: Some(repository_id),
            approval_id: Some(approval_id),
            work_item_id: None,
        }
    }

    pub const fn work_item(project_id: Uuid, work_item_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: None,
            run_id: None,
            repository_id: None,
            approval_id: None,
            work_item_id: Some(work_item_id),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegatedScope {
    scope: ProjectScope,
    allowed_actions: Vec<Action>,
}

impl DelegatedScope {
    pub fn new(scope: ProjectScope, allowed_actions: impl IntoIterator<Item = Action>) -> Self {
        let mut allowed_actions = allowed_actions.into_iter().collect::<Vec<_>>();
        allowed_actions.sort_unstable_by_key(|action| *action as u8);
        allowed_actions.dedup();

        Self {
            scope,
            allowed_actions,
        }
    }

    pub const fn scope(&self) -> ProjectScope {
        self.scope
    }

    pub fn allows(&self, action: Action, scope: ProjectScope) -> bool {
        self.scope == scope && self.allowed_actions.contains(&action)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subject {
    actor_id: Uuid,
    actor_kind: ActorKind,
    actor_active: bool,
    membership: Option<Membership>,
    delegated_scope: Option<DelegatedScope>,
}

impl Subject {
    pub const fn new(
        actor_id: Uuid,
        actor_kind: ActorKind,
        actor_active: bool,
        membership: Option<Membership>,
        delegated_scope: Option<DelegatedScope>,
    ) -> Self {
        Self {
            actor_id,
            actor_kind,
            actor_active,
            membership,
            delegated_scope,
        }
    }

    pub const fn actor_id(&self) -> Uuid {
        self.actor_id
    }

    pub const fn actor_kind(&self) -> ActorKind {
        self.actor_kind
    }

    pub const fn is_actor_active(&self) -> bool {
        self.actor_active
    }

    pub const fn membership(&self) -> Option<Membership> {
        self.membership
    }

    pub const fn delegated_scope(&self) -> Option<&DelegatedScope> {
        self.delegated_scope.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalEligibility {
    AnyAuthorizedMember,
    Actor(Uuid),
    Role(MembershipRole),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApprovalRequirements {
    pub requesting_actor_id: Uuid,
    pub eligibility: ApprovalEligibility,
    pub human_required: bool,
    pub separation_of_duty: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    Organization {
        organization_id: Uuid,
    },
    Project(ProjectScope),
    Approval {
        scope: ProjectScope,
        requirements: ApprovalRequirements,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenyReason {
    ActorDisabled,
    MembershipMissing,
    MembershipDisabled,
    ActionResourceMismatch,
    MembershipScopeMismatch,
    RoleDenied,
    DelegatedScopeMissing,
    DelegatedScopeMismatch,
    ApprovalEligibilityMismatch,
    HumanRequired,
    SeparationOfDuty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow,
    Deny(DenyReason),
}

impl Decision {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

pub fn authorize(subject: &Subject, action: Action, resource: Resource) -> Decision {
    if !subject.actor_active {
        return Decision::Deny(DenyReason::ActorDisabled);
    }

    let Some(membership) = subject.membership else {
        return Decision::Deny(DenyReason::MembershipMissing);
    };
    if !membership.enabled {
        return Decision::Deny(DenyReason::MembershipDisabled);
    }

    let resource_scope = match (action, resource) {
        (Action::OrganizationProjectCreate, Resource::Organization { organization_id }) => {
            MembershipScope::Organization(organization_id)
        }
        (
            Action::ProjectView
            | Action::ProjectManage
            | Action::InvitationManage
            | Action::MembershipView
            | Action::MembershipManage
            | Action::NavigationRead
            | Action::NavigationWrite
            | Action::WorkerInvoke
            | Action::RunInterrupt
            | Action::ChannelCreate
            | Action::MessageSend
            | Action::ThreadCreate
            | Action::ConversationArchive
            | Action::ReadStateUpdate
            | Action::MentionSearch
            | Action::WorkItemView
            | Action::WorkItemCreate
            | Action::WorkItemEdit
            | Action::RepositoryPublish
            | Action::PullRequestCreate
            | Action::PullRequestRefresh,
            Resource::Project(scope),
        ) => MembershipScope::Project(scope.project_id),
        (Action::ApprovalRespond, Resource::Approval { scope, .. }) => {
            MembershipScope::Project(scope.project_id)
        }
        _ => return Decision::Deny(DenyReason::ActionResourceMismatch),
    };

    if membership.scope != resource_scope {
        return Decision::Deny(DenyReason::MembershipScopeMismatch);
    }

    if membership.role == MembershipRole::Worker {
        if !worker_action_is_delegatable(action) {
            return Decision::Deny(DenyReason::RoleDenied);
        }

        let Some(delegated_scope) = subject.delegated_scope.as_ref() else {
            return Decision::Deny(DenyReason::DelegatedScopeMissing);
        };
        let Some(project_scope) = project_scope(resource) else {
            return Decision::Deny(DenyReason::RoleDenied);
        };
        if !delegated_scope.allows(action, project_scope) {
            return Decision::Deny(DenyReason::DelegatedScopeMismatch);
        }
    } else if !ordinary_role_allows(membership.role, action) {
        return Decision::Deny(DenyReason::RoleDenied);
    }

    if let Resource::Approval { requirements, .. } = resource {
        if !approval_eligibility_matches(
            subject.actor_id,
            membership.role,
            requirements.eligibility,
        ) {
            return Decision::Deny(DenyReason::ApprovalEligibilityMismatch);
        }
        if requirements.human_required && subject.actor_kind != ActorKind::Human {
            return Decision::Deny(DenyReason::HumanRequired);
        }
        if requirements.separation_of_duty && requirements.requesting_actor_id == subject.actor_id {
            return Decision::Deny(DenyReason::SeparationOfDuty);
        }
    }

    Decision::Allow
}

const fn ordinary_role_allows(role: MembershipRole, action: Action) -> bool {
    match role {
        MembershipRole::Owner | MembershipRole::Admin => true,
        MembershipRole::Member => matches!(
            action,
            Action::ProjectView
                | Action::NavigationRead
                | Action::NavigationWrite
                | Action::WorkerInvoke
                | Action::RunInterrupt
                | Action::ApprovalRespond
                | Action::ChannelCreate
                | Action::MessageSend
                | Action::ThreadCreate
                | Action::ConversationArchive
                | Action::ReadStateUpdate
                | Action::MentionSearch
                | Action::WorkItemView
                | Action::WorkItemCreate
                | Action::WorkItemEdit
                | Action::RepositoryPublish
                | Action::PullRequestCreate
                | Action::PullRequestRefresh
        ),
        MembershipRole::Viewer => {
            matches!(
                action,
                Action::ProjectView
                    | Action::NavigationRead
                    | Action::ReadStateUpdate
                    | Action::WorkItemView
            )
        }
        MembershipRole::Worker => false,
    }
}

const fn worker_action_is_delegatable(action: Action) -> bool {
    matches!(
        action,
        Action::ProjectView
            | Action::NavigationRead
            | Action::RunInterrupt
            | Action::ApprovalRespond
    )
}

const fn project_scope(resource: Resource) -> Option<ProjectScope> {
    match resource {
        Resource::Project(scope) | Resource::Approval { scope, .. } => Some(scope),
        Resource::Organization { .. } => None,
    }
}

fn approval_eligibility_matches(
    actor_id: Uuid,
    role: MembershipRole,
    eligibility: ApprovalEligibility,
) -> bool {
    match eligibility {
        ApprovalEligibility::AnyAuthorizedMember => true,
        ApprovalEligibility::Actor(eligible_actor_id) => actor_id == eligible_actor_id,
        ApprovalEligibility::Role(eligible_role) => role == eligible_role,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, ActorKind, ApprovalEligibility, ApprovalRequirements, Decision, DelegatedScope,
        DenyReason, Membership, MembershipRole, ProjectScope, Resource, Subject, authorize,
    };
    use uuid::Uuid;

    const ORGANIZATION_ID: Uuid = Uuid::from_u128(1);
    const PROJECT_ID: Uuid = Uuid::from_u128(2);
    const OTHER_PROJECT_ID: Uuid = Uuid::from_u128(3);
    const THREAD_ID: Uuid = Uuid::from_u128(4);
    const OTHER_THREAD_ID: Uuid = Uuid::from_u128(5);
    const RUN_ID: Uuid = Uuid::from_u128(6);
    const REPOSITORY_ID: Uuid = Uuid::from_u128(7);
    const APPROVAL_ID: Uuid = Uuid::from_u128(8);
    const ACTOR_ID: Uuid = Uuid::from_u128(9);
    const OTHER_ACTOR_ID: Uuid = Uuid::from_u128(10);

    fn project_scope() -> ProjectScope {
        ProjectScope::run(PROJECT_ID, THREAD_ID, RUN_ID, REPOSITORY_ID)
    }

    fn project_resource(action: Action, role: MembershipRole) -> Resource {
        match action {
            Action::OrganizationProjectCreate => Resource::Organization {
                organization_id: ORGANIZATION_ID,
            },
            Action::ApprovalRespond => Resource::Approval {
                scope: ProjectScope::approval(
                    PROJECT_ID,
                    THREAD_ID,
                    RUN_ID,
                    REPOSITORY_ID,
                    APPROVAL_ID,
                ),
                requirements: ApprovalRequirements {
                    requesting_actor_id: OTHER_ACTOR_ID,
                    eligibility: ApprovalEligibility::Role(role),
                    human_required: false,
                    separation_of_duty: false,
                },
            },
            _ => Resource::Project(project_scope()),
        }
    }

    fn membership_for(action: Action, role: MembershipRole, enabled: bool) -> Membership {
        match action {
            Action::OrganizationProjectCreate => {
                Membership::organization(ORGANIZATION_ID, role, enabled)
            }
            _ => Membership::project(PROJECT_ID, role, enabled),
        }
    }

    fn subject(
        kind: ActorKind,
        action: Action,
        role: MembershipRole,
        actor_active: bool,
        membership_enabled: bool,
        delegated_scope: Option<DelegatedScope>,
    ) -> Subject {
        Subject::new(
            ACTOR_ID,
            kind,
            actor_active,
            Some(membership_for(action, role, membership_enabled)),
            delegated_scope,
        )
    }

    fn worker_delegation(action: Action, resource: Resource) -> Option<DelegatedScope> {
        let scope = match resource {
            Resource::Project(scope) | Resource::Approval { scope, .. } => scope,
            Resource::Organization { .. } => return None,
        };
        Some(DelegatedScope::new(scope, [action]))
    }

    #[test]
    fn exhaustive_ordinary_role_action_matrix() {
        for role in MembershipRole::ALL {
            for action in Action::ALL {
                let resource = project_resource(action, role);
                let delegation = (role == MembershipRole::Worker)
                    .then(|| worker_delegation(action, resource))
                    .flatten();
                let decision = authorize(
                    &subject(ActorKind::Human, action, role, true, true, delegation),
                    action,
                    resource,
                );
                let expected = match role {
                    MembershipRole::Owner | MembershipRole::Admin => true,
                    MembershipRole::Member => matches!(
                        action,
                        Action::ProjectView
                            | Action::NavigationRead
                            | Action::NavigationWrite
                            | Action::WorkerInvoke
                            | Action::RepositoryPublish
                            | Action::PullRequestCreate
                            | Action::PullRequestRefresh
                            | Action::RunInterrupt
                            | Action::ApprovalRespond
                            | Action::ChannelCreate
                            | Action::MessageSend
                            | Action::ThreadCreate
                            | Action::ConversationArchive
                            | Action::ReadStateUpdate
                            | Action::MentionSearch
                            | Action::WorkItemView
                            | Action::WorkItemCreate
                            | Action::WorkItemEdit
                    ),
                    MembershipRole::Viewer => {
                        matches!(
                            action,
                            Action::ProjectView
                                | Action::NavigationRead
                                | Action::ReadStateUpdate
                                | Action::WorkItemView
                        )
                    }
                    MembershipRole::Worker => matches!(
                        action,
                        Action::ProjectView
                            | Action::NavigationRead
                            | Action::RunInterrupt
                            | Action::ApprovalRespond
                    ),
                };

                assert_eq!(
                    decision.is_allowed(),
                    expected,
                    "{role:?} performing {action:?}: {decision:?}"
                );
            }
        }
    }

    #[test]
    fn disabled_actor_denies_every_action_before_role_or_delegation() {
        for role in MembershipRole::ALL {
            for action in Action::ALL {
                let resource = project_resource(action, role);
                let delegation = worker_delegation(action, resource);
                let decision = authorize(
                    &subject(ActorKind::Human, action, role, false, true, delegation),
                    action,
                    resource,
                );
                assert_eq!(
                    decision,
                    Decision::Deny(DenyReason::ActorDisabled),
                    "{role:?} performing {action:?}"
                );
            }
        }
    }

    #[test]
    fn disabled_membership_denies_every_action_before_role_or_delegation() {
        for role in MembershipRole::ALL {
            for action in Action::ALL {
                let resource = project_resource(action, role);
                let delegation = worker_delegation(action, resource);
                let decision = authorize(
                    &subject(ActorKind::Human, action, role, true, false, delegation),
                    action,
                    resource,
                );
                assert_eq!(
                    decision,
                    Decision::Deny(DenyReason::MembershipDisabled),
                    "{role:?} performing {action:?}"
                );
            }
        }
    }

    #[test]
    fn membership_is_always_required() {
        for actor_kind in [
            ActorKind::Human,
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let subject = Subject::new(ACTOR_ID, actor_kind, true, None, None);

            assert_eq!(
                authorize(
                    &subject,
                    Action::ProjectView,
                    Resource::Project(ProjectScope::project(PROJECT_ID)),
                ),
                Decision::Deny(DenyReason::MembershipMissing),
                "{actor_kind:?}"
            );
        }
    }

    #[test]
    fn actor_kind_alone_never_creates_a_superuser_bypass() {
        for actor_kind in [
            ActorKind::Human,
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let viewer = subject(
                actor_kind,
                Action::ProjectManage,
                MembershipRole::Viewer,
                true,
                true,
                None,
            );

            assert_eq!(
                authorize(
                    &viewer,
                    Action::ProjectManage,
                    Resource::Project(ProjectScope::project(PROJECT_ID)),
                ),
                Decision::Deny(DenyReason::RoleDenied),
                "{actor_kind:?}"
            );
        }
    }

    #[test]
    fn human_and_external_agent_with_the_same_role_have_identical_authority() {
        for role in [
            MembershipRole::Owner,
            MembershipRole::Admin,
            MembershipRole::Member,
            MembershipRole::Viewer,
        ] {
            for action in Action::ALL {
                let resource = project_resource(action, role);
                let human = subject(ActorKind::Human, action, role, true, true, None);
                let external_agent =
                    subject(ActorKind::ExternalAgent, action, role, true, true, None);

                assert_eq!(
                    authorize(&human, action, resource),
                    authorize(&external_agent, action, resource),
                    "{role:?} performing {action:?}"
                );
            }
        }
    }

    #[test]
    fn worker_requires_an_explicit_delegation() {
        let subject = subject(
            ActorKind::ExecutionWorker,
            Action::ProjectView,
            MembershipRole::Worker,
            true,
            true,
            None,
        );

        assert_eq!(
            authorize(
                &subject,
                Action::ProjectView,
                Resource::Project(project_scope()),
            ),
            Decision::Deny(DenyReason::DelegatedScopeMissing)
        );
    }

    #[test]
    fn worker_delegation_must_match_the_action() {
        let scope = project_scope();
        let subject = subject(
            ActorKind::ExecutionWorker,
            Action::ProjectView,
            MembershipRole::Worker,
            true,
            true,
            Some(DelegatedScope::new(scope, [Action::NavigationRead])),
        );

        assert_eq!(
            authorize(&subject, Action::ProjectView, Resource::Project(scope),),
            Decision::Deny(DenyReason::DelegatedScopeMismatch)
        );
    }

    #[test]
    fn worker_delegation_must_match_every_resource_identifier() {
        let delegated = project_scope();
        let subject = subject(
            ActorKind::ExecutionWorker,
            Action::ProjectView,
            MembershipRole::Worker,
            true,
            true,
            Some(DelegatedScope::new(delegated, [Action::ProjectView])),
        );
        let mismatches = [
            ProjectScope {
                project_id: OTHER_PROJECT_ID,
                ..delegated
            },
            ProjectScope {
                thread_id: Some(OTHER_THREAD_ID),
                ..delegated
            },
            ProjectScope {
                run_id: Some(Uuid::from_u128(11)),
                ..delegated
            },
            ProjectScope {
                repository_id: Some(Uuid::from_u128(12)),
                ..delegated
            },
            ProjectScope {
                approval_id: Some(APPROVAL_ID),
                ..delegated
            },
            ProjectScope {
                work_item_id: Some(Uuid::from_u128(13)),
                ..delegated
            },
            ProjectScope::project(PROJECT_ID),
        ];

        for mismatched in mismatches {
            let decision = authorize(&subject, Action::ProjectView, Resource::Project(mismatched));
            let expected = if mismatched.project_id == OTHER_PROJECT_ID {
                Decision::Deny(DenyReason::MembershipScopeMismatch)
            } else {
                Decision::Deny(DenyReason::DelegatedScopeMismatch)
            };
            assert_eq!(decision, expected, "{mismatched:?}");
        }
    }

    #[test]
    fn worker_cannot_delegate_project_management_or_worker_invocation() {
        for action in [
            Action::OrganizationProjectCreate,
            Action::ProjectManage,
            Action::NavigationWrite,
            Action::WorkerInvoke,
        ] {
            let resource = project_resource(action, MembershipRole::Worker);
            let delegation = worker_delegation(action, resource);
            let worker = subject(
                ActorKind::ExecutionWorker,
                action,
                MembershipRole::Worker,
                true,
                true,
                delegation,
            );

            assert_eq!(
                authorize(&worker, action, resource),
                Decision::Deny(DenyReason::RoleDenied),
                "{action:?}"
            );
        }
    }

    #[test]
    fn cross_scope_memberships_do_not_authorize() {
        let project_member = Subject::new(
            ACTOR_ID,
            ActorKind::Human,
            true,
            Some(Membership::project(PROJECT_ID, MembershipRole::Owner, true)),
            None,
        );
        assert_eq!(
            authorize(
                &project_member,
                Action::OrganizationProjectCreate,
                Resource::Organization {
                    organization_id: ORGANIZATION_ID,
                },
            ),
            Decision::Deny(DenyReason::MembershipScopeMismatch)
        );

        let organization_member = Subject::new(
            ACTOR_ID,
            ActorKind::Human,
            true,
            Some(Membership::organization(
                ORGANIZATION_ID,
                MembershipRole::Owner,
                true,
            )),
            None,
        );
        assert_eq!(
            authorize(
                &organization_member,
                Action::ProjectView,
                Resource::Project(ProjectScope::project(PROJECT_ID)),
            ),
            Decision::Deny(DenyReason::MembershipScopeMismatch)
        );
    }

    #[test]
    fn action_and_resource_must_be_compatible() {
        let owner = subject(
            ActorKind::Human,
            Action::ProjectManage,
            MembershipRole::Owner,
            true,
            true,
            None,
        );

        assert_eq!(
            authorize(
                &owner,
                Action::ProjectManage,
                Resource::Organization {
                    organization_id: ORGANIZATION_ID,
                },
            ),
            Decision::Deny(DenyReason::ActionResourceMismatch)
        );
        assert_eq!(
            authorize(
                &owner,
                Action::ApprovalRespond,
                Resource::Project(ProjectScope::project(PROJECT_ID)),
            ),
            Decision::Deny(DenyReason::ActionResourceMismatch)
        );
    }

    fn approval_resource(
        eligibility: ApprovalEligibility,
        human_required: bool,
        separation_of_duty: bool,
        requesting_actor_id: Uuid,
    ) -> Resource {
        Resource::Approval {
            scope: ProjectScope::approval(
                PROJECT_ID,
                THREAD_ID,
                RUN_ID,
                REPOSITORY_ID,
                APPROVAL_ID,
            ),
            requirements: ApprovalRequirements {
                requesting_actor_id,
                eligibility,
                human_required,
                separation_of_duty,
            },
        }
    }

    fn approval_subject(kind: ActorKind, actor_id: Uuid, role: MembershipRole) -> Subject {
        Subject::new(
            actor_id,
            kind,
            true,
            Some(Membership::project(PROJECT_ID, role, true)),
            None,
        )
    }

    #[test]
    fn approval_must_be_addressed_to_the_actor_or_their_exact_role() {
        let member = approval_subject(ActorKind::Human, ACTOR_ID, MembershipRole::Member);

        assert!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(ACTOR_ID),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            )
            .is_allowed()
        );
        assert!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Role(MembershipRole::Member),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            )
            .is_allowed()
        );
        assert_eq!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(OTHER_ACTOR_ID),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            ),
            Decision::Deny(DenyReason::ApprovalEligibilityMismatch)
        );
        assert_eq!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Role(MembershipRole::Owner),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            ),
            Decision::Deny(DenyReason::ApprovalEligibilityMismatch)
        );
    }

    #[test]
    fn viewer_cannot_answer_even_when_explicitly_addressed() {
        let viewer = approval_subject(ActorKind::Human, ACTOR_ID, MembershipRole::Viewer);

        assert_eq!(
            authorize(
                &viewer,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(ACTOR_ID),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            ),
            Decision::Deny(DenyReason::RoleDenied)
        );
    }

    #[test]
    fn human_required_approval_rejects_every_non_human_actor_kind() {
        for actor_kind in [
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let subject = approval_subject(actor_kind, ACTOR_ID, MembershipRole::Member);

            assert_eq!(
                authorize(
                    &subject,
                    Action::ApprovalRespond,
                    approval_resource(
                        ApprovalEligibility::Actor(ACTOR_ID),
                        true,
                        false,
                        OTHER_ACTOR_ID,
                    ),
                ),
                Decision::Deny(DenyReason::HumanRequired),
                "{actor_kind:?}"
            );
        }

        let human = approval_subject(ActorKind::Human, ACTOR_ID, MembershipRole::Member);
        assert!(
            authorize(
                &human,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(ACTOR_ID),
                    true,
                    false,
                    OTHER_ACTOR_ID,
                ),
            )
            .is_allowed()
        );
    }

    #[test]
    fn separation_of_duty_rejects_self_approval_for_every_actor_kind() {
        for actor_kind in [
            ActorKind::Human,
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let subject = approval_subject(actor_kind, ACTOR_ID, MembershipRole::Member);

            assert_eq!(
                authorize(
                    &subject,
                    Action::ApprovalRespond,
                    approval_resource(ApprovalEligibility::Actor(ACTOR_ID), false, true, ACTOR_ID,),
                ),
                Decision::Deny(DenyReason::SeparationOfDuty),
                "{actor_kind:?}"
            );
        }
    }

    #[test]
    fn explicitly_delegated_worker_approval_still_obeys_approval_conditions() {
        let resource = approval_resource(
            ApprovalEligibility::Actor(ACTOR_ID),
            true,
            false,
            OTHER_ACTOR_ID,
        );
        let scope = match resource {
            Resource::Approval { scope, .. } => scope,
            _ => unreachable!("test resource is an approval"),
        };
        let worker = Subject::new(
            ACTOR_ID,
            ActorKind::ExecutionWorker,
            true,
            Some(Membership::project(
                PROJECT_ID,
                MembershipRole::Worker,
                true,
            )),
            Some(DelegatedScope::new(scope, [Action::ApprovalRespond])),
        );

        assert_eq!(
            authorize(&worker, Action::ApprovalRespond, resource),
            Decision::Deny(DenyReason::HumanRequired)
        );
    }
}
