# ADR 0015: GitHub Projects stay downstream of Issues

## Context

Users may organize Orbit-created Issues in GitHub Projects. Projects add a second
state model: membership, a built-in Status field, custom fields, views, and
workflows. An Issue can belong to several Projects, each with a different Status.
Orbit already owns Task workflow state and synchronizes the repository Issue:
title, body, labels, open/closed state, and organization Issue Fields.

Making Project Status authoritative would require choosing one Project among
possibly several, mapping customizable Status options to Orbit's four statuses,
requesting Project scopes, and defining new conflict rules. None of those choices
has a single correct answer across Projects.

## Decision

- Orbit's GitHub integration boundary is the Issue, identified by
  `owner/repo#number`.
- Orbit does not call the ProjectV2 API, manage Project membership, or read or
  write Project items, Status, views, or custom fields.
- Projects are downstream reporting. Users may rely on Project auto-add (or add
  Issues manually) and built-in workflows that mirror Issue closed/reopened
  state.
- Project custom fields are not organization Issue Fields. Orbit does not alias
  fields by name between the two systems.
- Project draft issues are not repository Issues and are invisible to Orbit
  until converted.
- A future membership-only integration or field mapping requires a new decision.
  If Project field synchronization is ever added, exactly one Project must be
  explicitly authoritative.

## Consequences

- Orbit needs no `project` or `read:project` scope and no Project configuration.
- Without the separate **Auto-close issue** workflow, a Project card move does
  not change the Issue or the Orbit Task. If that workflow is enabled, moving an
  item to its closing Status closes the Issue; the next pull sync sees only the
  Issue change and can update the Task when Orbit's own status is unchanged.
  Orbit never reads Project Status.
- Project Status can lag Issue state unless a Project workflow updates it.
- Project Status cannot represent the difference between `todo` and
  `in_progress`, or between `done` and `cancelled`; labels and views carry those
  distinctions when needed.
- Multiple Projects can contain the same Issue without creating duplicate Orbit
  links or Tasks.
