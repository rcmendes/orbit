# Agent brief

Read the agent brief before you implement a Ready ticket. The Task body and the comment thread are context. The brief is the contract.

Matt Pocock's `triage` skill writes that brief when a ticket moves to `ready-for-agent`. The writing convention is that skill's `AGENT-BRIEF.md`: describe behavior that stays true after files move, skip paths and line numbers, and make every acceptance criterion testable on its own. Orbit does not write briefs. `orbit triage` only sets `area`, task-level `verification`, and canonical labels.

Post the brief as a comment on the Task. Start the comment with the triage disclaimer in [`issue-tracker.md`](./issue-tracker.md).

## Template

```markdown
## Agent brief

**Goal:** what is true when this ticket is done.

**Scope:** the behavior this ticket includes.

**Non-goals:** adjacent work this ticket leaves alone.

**Acceptance criteria:**
- [ ] A check a reviewer can run or observe
- [ ] Another independent check

**Verification:** the command or procedure that shows the criteria hold.
```
