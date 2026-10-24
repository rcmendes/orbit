# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's Orbit board.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

## Category labels

- `bug`: something is broken
- `enhancement`: new feature or improvement

## Orbit semantics

`needs-triage`, `needs-info`, and `ready-for-human` are **hold labels**. A task carrying one is excluded from `orbit ready` and `orbit claim` returns `LABEL_BLOCKED`. Remove the hold label before claiming the task.

`ready-for-agent`, `bug`, and `enhancement` do not affect readiness.

`wontfix` is a label plus a terminal status: add the label and run `orbit cancel <ID> --by <agent>`.

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from the table, then remove the previous state label.
