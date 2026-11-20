//! Pages, skills and page discussion as operations (K2.10): reading and
//! listing pages and skills, making and changing pages (whole or block by
//! block), and a page's comments and suggested edits.
//!
//! Each runs the command its hand-written tool ran (`ops::wrapped`), as the
//! caller the dispatcher checked, so how someone may write to a page (edit
//! it, or only suggest) is still decided inside, by their role there.
//! `page_write` became `page_create` and `page_update`; the old names stay
//! as aliases (`ops::aliases`).

use uuid::Uuid;

use crate::{
    mcp::{memory_tools, page_discussion},
    ops::{
        Registry, Target, Targeted,
        refs::parse_id,
        wrapped::{names_existing, run_tool, tool_op},
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<PageList>()
        .add::<PageGet>()
        .add::<PageBlocksGet>()
        .add::<PageCreate>()
        .add::<PageUpdate>()
        .add::<PageUpdateBlocks>()
        .add::<SkillList>()
        .add::<SkillGet>()
        .add::<PageCommentList>()
        .add::<PageCommentCreate>()
        .add::<PageCommentUpdate>()
        .add::<PageSuggestionList>()
        .add::<PageSuggestionDecide>();
}

fn space(project_id: &str) -> Target {
    Target::Space(parse_id(project_id).unwrap_or_else(Uuid::nil))
}

fn page(page_id: &str) -> Target {
    Target::thing("page", parse_id(page_id).unwrap_or_else(Uuid::nil))
}

fn space_or_workspace(project_id: Option<&str>) -> Target {
    match project_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(project) => space(project),
        None => Target::Workspace(None),
    }
}

macro_rules! on_page {
    ($($arguments:ty),* $(,)?) => {
        $(
            impl Targeted for $arguments {
                fn target(&self) -> Target {
                    page(&self.page_id)
                }
            }
        )*
    };
}

on_page!(
    memory_tools::PageGetArguments,
    memory_tools::PageBlocksArguments,
    memory_tools::PageEditArguments,
    page_discussion::PageCommentsArguments,
    page_discussion::PageCommentArguments,
    page_discussion::PageCommentUpdateArguments,
    page_discussion::PageSuggestionsArguments,
    page_discussion::PageSuggestionDecideArguments,
);

impl Targeted for memory_tools::PageListArguments {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Targeted for memory_tools::SkillListArguments {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Targeted for memory_tools::SkillGetArguments {
    fn target(&self) -> Target {
        match self.page_id.as_deref() {
            Some(page_id) => page(page_id),
            None => space_or_workspace(self.project_id.as_deref()),
        }
    }
}

impl Targeted for memory_tools::PageWriteArguments {
    fn target(&self) -> Target {
        match self.page_id.as_deref() {
            Some(page_id) => page(page_id),
            None => space_or_workspace(self.project_id.as_deref()),
        }
    }
}

// ---------------------------------------------------------------------------
// Pages

tool_op!(PageList {
    name: "page_list",
    module: "pages",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List pages",
    description: "List a space's pages, or with projectId \"workspace\" the workspace's own \
        pages: the guide first, then the rest by last edit. Stale pages haven't been touched \
        in months.",
    emits: [],
    input: memory_tools::PageListArguments,
    output: memory_tools::PageListValue,
    run: |call, input| { run_tool!(call, memory_tools::page_list, input) }
});

tool_op!(PageGet {
    name: "page_get",
    module: "pages",
    kind: Describe,
    permission: None,
    idempotency: None,
    resource: crate::ops::ResourceAddress {
        path: "pages/{pageId}",
        mime_type: "text/markdown",
        text_at: Some("/body"),
    },
    title: "Read page",
    description: "Read a page (or an earlier version of it), with its recent history.",
    emits: [],
    input: memory_tools::PageGetArguments,
    output: memory_tools::PageValue,
    run: |call, input| { run_tool!(call, memory_tools::page_get, input) }
});

tool_op!(PageBlocksGet {
    name: "page_blocks_get",
    module: "pages",
    kind: Describe,
    permission: None,
    idempotency: None,
    title: "Read page blocks",
    description: "Read a page as blocks: its outline and each block's lasting id, type and \
        hash. Pass section (a heading's id) or from/to (block ids) to read exactly that part; \
        positions work too, with the version they came from. Change blocks with \
        page_update_blocks rather than rewriting the page.",
    emits: [],
    input: memory_tools::PageBlocksArguments,
    output: memory_tools::PageBlocksValue,
    run: |call, input| { run_tool!(call, memory_tools::page_blocks, input) }
});

tool_op!(PageCreate {
    name: "page_create",
    module: "pages",
    kind: Create,
    permission: Some(crate::ops::Perm::PagesSuggest),
    idempotency: Dispatcher,
    audited: true,
    title: "Create page",
    description: "Make a page: what's true now for a space (or, with projectId \"workspace\", \
        the whole workspace), in Markdown. With skillDescription it is a skill, loaded with \
        skill_get when that line says it applies. A line `[label](/path \"embed\")` shows the \
        thing at that address live. fromTemplateId starts it from a template.",
    emits: ["page.updated"],
    input: memory_tools::PageWriteArguments,
    output: memory_tools::PageValue,
    run: |call, input| {
        names_existing(input.page_id.is_some(), true, "pageId")?;
        run_tool!(call, anchored, memory_tools::page_write, input)
    }
});

tool_op!(PageUpdate {
    name: "page_update",
    module: "pages",
    kind: Change,
    permission: Some(crate::ops::Perm::PagesSuggest),
    idempotency: Dispatcher,
    audited: true,
    title: "Update page",
    description: "Change a page: new Markdown (replacing the text; every earlier version is \
        kept and restoreVersion brings one back), its title, guide, template or skill line, or \
        archive or restore it. Pass expectedVersion. With only the right to suggest on pages, \
        the change is kept as a suggestion (the answer says so).",
    emits: ["page.updated"],
    input: memory_tools::PageWriteArguments,
    output: memory_tools::PageValue,
    run: |call, input| {
        names_existing(input.page_id.is_some(), false, "pageId")?;
        run_tool!(call, memory_tools::page_write, input)
    }
});

tool_op!(PageUpdateBlocks {
    name: "page_update_blocks",
    module: "pages",
    kind: Change,
    permission: Some(crate::ops::Perm::PagesSuggest),
    idempotency: Dispatcher,
    audited: true,
    title: "Edit page blocks",
    description: "Change a page block by block: replace, insertBefore, insertAfter or delete \
        blocks named by the id and hash page_blocks_get gave, or append. The rest stays as it \
        was, and people writing at the same time keep what they typed. Edits by position need \
        expectedVersion. Where you may only suggest, or with suggest: true, the edits wait as a \
        suggestion.",
    emits: ["page.updated"],
    input: memory_tools::PageEditArguments,
    output: memory_tools::PageBlocksValue,
    run: |call, input| { run_tool!(call, memory_tools::page_edit, input) }
});

// ---------------------------------------------------------------------------
// Skills

tool_op!(SkillList {
    name: "skill_list",
    module: "pages",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List skills",
    description: "List a space's skills: pages of instructions for a kind of work, each with \
        one line saying when to use it. Check them when you start work in a space and load the \
        ones that fit with skill_get.",
    emits: [],
    input: memory_tools::SkillListArguments,
    output: memory_tools::SkillListValue,
    run: |call, input| { run_tool!(call, memory_tools::skill_list, input) }
});

tool_op!(SkillGet {
    name: "skill_get",
    module: "pages",
    kind: Describe,
    permission: None,
    idempotency: None,
    title: "Load skill",
    description: "Load a skill by name (with projectId) or pageId, and follow it. Also \
        returns it as a SKILL.md file, for harnesses that keep skills on disk.",
    emits: [],
    input: memory_tools::SkillGetArguments,
    output: memory_tools::SkillValue,
    run: |call, input| {
        if input.page_id.is_some() {
            run_tool!(call, memory_tools::skill_get, input)
        } else {
            run_tool!(call, anchored, memory_tools::skill_get, input)
        }
    }
});

// ---------------------------------------------------------------------------
// Page discussion

tool_op!(PageCommentList {
    name: "page_comment_list",
    module: "pages",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "Page comments",
    description: "A page's comment threads: each with the block and words it's about \
        (anchorFound is false once they're edited away), its replies, and whether it's \
        resolved (resolved ones only with includeResolved).",
    emits: [],
    input: page_discussion::PageCommentsArguments,
    output: page_discussion::PageCommentsValue,
    run: |call, input| { run_tool!(call, page_discussion::page_comments, input) }
});

tool_op!(PageCommentCreate {
    name: "page_comment_create",
    module: "pages",
    kind: Create,
    permission: Some(crate::ops::Perm::PagesComment),
    idempotency: Dispatcher,
    audited: true,
    title: "Comment on page",
    description: "Comment on a page: start a thread about exact words in a block (blockId \
        and quote), about the whole page, or reply in a thread (replyTo). The page's maker and \
        the thread's people are told; a reply reopens a resolved thread.",
    emits: ["page.commented"],
    input: page_discussion::PageCommentArguments,
    output: page_discussion::PageCommentValue,
    run: |call, input| { run_tool!(call, page_discussion::page_comment, input) }
});

tool_op!(PageCommentUpdate {
    name: "page_comment_update",
    module: "pages",
    kind: Change,
    permission: Some(crate::ops::Perm::PagesComment),
    idempotency: Dispatcher,
    destructive: true,
    audited: true,
    title: "Update page comment",
    description: "Edit or delete your own page comment (text, delete: true), or resolve or \
        reopen its thread (resolved).",
    emits: ["page.comment_changed"],
    input: page_discussion::PageCommentUpdateArguments,
    output: page_discussion::PageCommentUpdateValue,
    run: |call, input| { run_tool!(call, page_discussion::page_comment_update, input) }
});

tool_op!(PageSuggestionList {
    name: "page_suggestion_list",
    module: "pages",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "Suggested edits",
    description: "The edits suggested for a page, waiting for someone who may edit it to \
        accept or reject: each with its block edits, the Markdown before and the new Markdown.",
    emits: [],
    input: page_discussion::PageSuggestionsArguments,
    output: page_discussion::PageSuggestionsValue,
    run: |call, input| { run_tool!(call, page_discussion::page_suggestions, input) }
});

tool_op!(PageSuggestionDecide {
    name: "page_suggestion_decide",
    module: "pages",
    kind: Change,
    permission: Some(crate::ops::Perm::PagesSuggest),
    idempotency: Dispatcher,
    audited: true,
    title: "Decide suggested edit",
    description: "Accept (applied as its author's edit) or reject a suggested edit on a page \
        you may edit, or withdraw your own. Accepting is refused if the blocks it changes were \
        edited since.",
    emits: ["page.updated"],
    input: page_discussion::PageSuggestionDecideArguments,
    output: page_discussion::PageSuggestionDecideValue,
    run: |call, input| { run_tool!(call, page_discussion::page_suggestion_decide, input) }
});
