---
title: Align Wayfinder labels and claims with Matt Pocock contract
status: done
type: task
priority: 2
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-dkyngc.z1wgx4
parent: orbit-dkyngc
area: docs
created_at: 2026-10-28T15:20:13.591613Z
updated_at: 2026-10-28T15:51:55.679772Z
---

## Goal

Make Orbit's Wayfinder operations match the contract Matt Pocock's `wayfinder` skill expects, without changing Orbit's claim model.

## Scope

- Update the Wayfinding operations in `docs/agents/issue-tracker.md`.
- A map is a Parent Task labelled `wayfinder:map`.
- Each child ticket is a SubTask created with `--parent <map-id>` and labelled `wayfinder:<type>`, where type is one of `research`, `prototype`, `grilling`, or `task`.
- Use the parent edge for map membership and `orbit ready --parent <map-id>` or `orbit list --parent <map-id>` for scoping. Do not use `wayfinder:<map-id>` as a label.
- Document claiming with `orbit claim <id> --by <agent>` or `orbit ready --parent <map-id> --claim --by <agent>`.
- State explicitly that GitHub assignees are not Orbit claims and are not used for Wayfinder ownership.
- Keep blocking edges on `--depends-on` / `orbit depend` and resolution on `orbit done` or `orbit cancel`.

## Acceptance criteria

- The docs no longer instruct agents to use `wayfinder:<map-id>`.
- Child tickets use the four `wayfinder:<type>` labels.
- Map scoping uses the parent edge, not a per-map label.
- Claiming uses Orbit leases; GitHub assignees are documented as unrelated.
- A reader can chart a map, create children, wire blockers, claim a ticket, and resolve it using the documented commands.

## Verification

- Confirm `wayfinder:<map-id>` no longer appears in the tracker docs.
- Exercise the documented commands against a scratch Board or dry-run fixture.
