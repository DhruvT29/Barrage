---
name: writing-plans
description: Use when turning a BARRAGE phase from PLAN.md into an implementation plan of tickable tasks, before touching code
---

<!--
Adapted from the `writing-plans` skill in obra/superpowers
https://github.com/obra/superpowers/tree/main/skills/writing-plans
Licensed MIT. Copyright (c) Jesse Vincent and contributors.

Local changes: plan save path points at this project, code examples use
Rust/cargo instead of Python/pytest, and the execution handoff creates Jira
tickets instead of dispatching subagents. Sibling-skill references removed.
-->

# Writing Plans

## Overview

Write implementation plans for an engineer who has not seen this codebase or this spec. Assume they write idiomatic code in the project's language once they know the exact interface and the exact test, and that they will make a reasonable choice wherever the plan leaves one open. What they cannot know is what you decided: which files, which names and signatures, which values from the spec, which tests prove each task. Document those. Give them the whole plan as bite-sized tasks. DRY. YAGNI. TDD. Frequent commits.

**Announce at start:** "I'm using the writing-plans skill to create the implementation plan."

**The spec for this project is `plans/PLAN.md`.** A phase in its section 5 is one plan's worth of work. Do not re-argue anything in its section 6 (rejected approaches).

**Save plans to:** `plans/YYYY-MM-DD-phase-N-<name>.md`, beside the spec.

## Scope Check

One plan per PLAN.md phase. If a phase turns out to cover two independent subsystems, split it into two plans and say so. Each plan must produce working, testable software on its own.

## File Structure

Before defining tasks, map out which files will be created or modified and what each one is responsible for. This is where decomposition decisions get locked in.

- Design units with clear boundaries and well-defined interfaces. Each file should have one clear responsibility.
- You reason best about code you can hold in context at once, and your edits are more reliable when files are focused. Prefer smaller, focused files over large ones that do too much.
- Files that change together should live together. Split by responsibility, not by technical layer.
- Follow patterns already established in the repo. If a file you are modifying has grown unwieldy, including a split in the plan is reasonable.

This structure informs the task decomposition. Each task should produce self-contained changes that make sense independently.

## Task Right-Sizing

A task is the smallest unit that carries its own test cycle and is worth a fresh reviewer's gate. When drawing task boundaries: fold setup, configuration, scaffolding, and documentation steps into the task whose deliverable needs them; split only where a reviewer could meaningfully reject one task while approving its neighbor. Each task ends with an independently testable deliverable.

## Step Granularity

**Each step is one action with a checkable result:**
- "Write the failing test" - step
- "Run it to make sure it fails" - step
- "Implement the minimal code to make the test pass" - step
- "Run the tests and make sure they pass" - step
- "Commit" - step

## Plan Document Header

**Every plan MUST start with this header:**

```markdown
# Phase N: [Name] Implementation Plan

> Steps use checkbox (`- [ ]`) syntax. Each task becomes one Jira issue;
> each step becomes a line in that issue's description.

**Goal:** [One sentence describing what this builds]

**Architecture:** [2-3 sentences about approach]

**Spec:** PLAN.md section 5, phase N. Acceptance criteria are quoted verbatim
below and are the definition of done.

## Global Constraints

[PLAN.md section 3.7's hard rules, plus anything phase-specific. Exact values
copied verbatim from PLAN.md. Every task's requirements implicitly include
this section.]

## Acceptance Criteria (from PLAN.md)

[Quoted verbatim. Every line here must be exercised by some task's tests.]

## Review Focus

[The five input classes or failure modes the spec implies but no task's tests
exercise that are most likely to bite a person using this software, one line
each, naming the input or condition and the behavior a reasonable person would
expect, most likely first. The spec says what the software must do, not
everything it will meet, and its silence on an input is not permission for
that input to break the program. Write the list here once, with PLAN.md in
front of you. Then, for each line, add the test that pins it to the task that
owns the code, in that task's own step style.]

---
```

## Task Structure

````markdown
### Task N: [Component Name]

**Files:**
- Create: `engine/src/links/enumerate.rs`
- Modify: `engine/src/links/mod.rs:1-12`
- Test: `engine/tests/links_enumerate.rs`

**Interfaces:**
- Consumes: [what this task uses from earlier tasks, exact signatures]
- Produces: [what later tasks rely on, exact function names, parameter and
  return types. A task's implementer sees only their own task; this block is
  how they learn the names and types neighbouring tasks use.]

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn rejects_link_local_addresses() {
    let got = candidates_from(&[iface("Ethernet", "169.254.123.14")]);
    assert!(got.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test links_enumerate rejects_link_local`
Expected: FAIL, `candidates_from` not found

- [ ] **Step 3: Implement `candidates_from(&[Iface]) -> Vec<Candidate>` in `engine/src/links/enumerate.rs`**

One line on the approach when the signature and the test leave a choice
(which crate call, which data structure); a code block only for an algorithm
they do not determine.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test links_enumerate rejects_link_local`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add engine/src/links/enumerate.rs engine/tests/links_enumerate.rs
git commit -m "feat(links): reject link-local addresses during enumeration"
```
````

## What a Step Contains

A step is done when the implementer can write exactly one reasonable thing from it. That is the whole requirement: unambiguous, not complete. Each kind of step carries what makes it unambiguous and nothing more:

- **A test step:** the test's name and its assertions, as code, with the spec's exact values in them.
- **A code step:** the exact signature (name, parameters, return type), the file it lives in, and the specific values the spec pins. The implementer writes the body. A body appears only for an algorithm the signature and tests do not determine, or for exact copy the spec fixes.
- **A verification step:** the command to run and the output that means it passed.
- **A reference to another task:** that task's Interfaces block says what to use; the plan does not repeat that task's code.

A plan is the set of decisions the implementer cannot make alone. A plan longer than the code it describes has written the code instead. Lines that decide nothing ("TBD", "handle edge cases", "add appropriate validation", "write tests for the above", a type or function no task defines) are the opposite failure, and the self-review catches both.

## Self-Review

After writing the complete plan, look at PLAN.md with fresh eyes and check the plan against it. This is a checklist you run yourself.

**1. Spec coverage:** Skim the phase's work items and acceptance criteria in PLAN.md. Can you point to a task that implements each? List any gaps.

**2. Step scan:** Every step must let the implementer write exactly one reasonable thing, and no step may carry more than that: a line that decides nothing is a gap, a function body the signature and tests already determine is a transcript. Fix both.

**3. Type consistency:** Do the types, signatures and names used in later tasks match what earlier tasks defined? A function called `candidates()` in Task 1 but `list_candidates()` in Task 4 is a bug.

**4. Review Focus:** For each input class or failure mode the spec implies, is there a task whose tests exercise it? The five uncovered ones most likely to bite a person go in Review Focus, and each line there gets its test added to the owning task. An empty section means you checked and found none, not that you skipped the check.

**5. Proportion:** Compare the plan's length to the phase's section in PLAN.md. If code blocks are most of the document, replace bodies with signatures, test names and assertions, and check each step is still unambiguous.

**6. Hard rules:** grep the plan for `192.168`, `10.`, `172.`. Any hit outside a comment or an example violates PLAN.md section 3.7.

If you find issues, fix them inline. No need to re-review. If you find a PLAN.md requirement with no task, add the task.

## Handoff

After saving and self-reviewing the plan:

1. Show the task list to the user as a numbered summary, one line per task, with the file each one touches. Do not create anything yet.
2. Wait for their approval or edits.
3. On approval, generate `plans/phaseN-jira-batch.json` and create the issues with `jira/jira.ps1 -CreateBatch -Project BRG`: one Epic for the phase, one Task per plan task, each Task's description carrying its steps as a checklist.

The plan file is canonical. If a task changes, edit the plan, regenerate the batch file, then update the issue. Never hand-edit a ticket into disagreeing with its plan.
4. Report the created issue keys and link the plan file.
