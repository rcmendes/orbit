# Triage Labels

The skills speak in terms of five canonical triage roles. On a Board with
the `mattpocock` profile applied (`orbit setup mattpocock`), each role maps
onto the Issue workflow — a status reached through `orbit transition`, an
assignee kind, a resolution, or an Issue type — instead of a label.

| Label in mattpocock/skills | Orbit mechanism | Meaning                                  |
| -------------------------- | --------------- | ---------------------------------------- |
| `needs-triage`             | `orbit transition <ID> needs-triage` → status `Triage` | Maintainer needs to evaluate this issue  |
| `needs-info`               | `orbit transition <ID> needs-info` → status `Needs Info` | Waiting on reporter for more information |
| `ready-for-agent`          | `orbit transition <ID> ready-for-agent` → status `Ready` | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `orbit transition <ID> ready-for-human` → status `Ready`, then `orbit assign <ID> --to <name>` | Requires human implementation            |
| `wontfix`                  | `orbit transition <ID> wontfix` → status `Cancelled`, resolution `Won't Do` | Will not be actioned                     |

## Category roles → Issue types

- `bug` → Issue type `bug`: `orbit add "<title>" --type bug` or
  `orbit update <ID> --type bug`
- `enhancement` → Issue type `story`

## Orbit semantics

Only the `Ready` status lists in `orbit ready`, and only while the Issue is
unassigned, unblocked, and free of hold labels. `Triage` and `Needs Info`
Issues are never Ready, so they cannot be claimed. A `ready-for-human` Issue
is assigned to a person with `orbit assign <ID> --to <name>` — the assignee
kind is `human` — which takes it out of the agent queue.

`wontfix` closes the Issue through the workflow's `wontfix` transition,
which records resolution `Won't Do`; `orbit reopen <ID>` returns it to the
initial status (`Triage`).

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), run
the matching `orbit transition`. Each transition replaces the previous
triage state, so there is no label to remove first.
