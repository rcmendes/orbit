use chrono::{TimeZone, Utc};
use orbit_tracker::graph::Graph;
use orbit_tracker::task::{Link, Task, TaskFrontmatter};
use orbit_tracker::workflow::Workflow;
use proptest::prelude::*;
use std::collections::{BTreeMap, HashSet};

fn task(id: String, status: &str, dependencies: Vec<String>) -> Task {
    let now = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
    Task {
        id,
        front: TaskFrontmatter {
            title: "property task".to_string(),
            status: status.to_string(),
            key: None,
            key_aliases: Vec::new(),
            resolution: None,
            task_type: "task".to_string(),
            priority: "medium".to_string(),
            labels: Vec::new(),
            links: dependencies
                .into_iter()
                .map(|dependency| Link::dependency(&dependency))
                .collect(),
            parent: None,
            checkpoint: None,
            component: None,
            verification: None,
            github: None,
            remote: None,
            repository: None,
            external_id: None,
            assignee: None,
            lease: None,
            reporter: None,
            created_at: now,
            updated_at: now,
            source_key: None,
            properties: BTreeMap::new(),
            extra: BTreeMap::new(),
        },
        body: String::new(),
    }
}

fn build_dag(edges: &[(usize, usize)], statuses: &[&str]) -> (Graph, Vec<String>) {
    let ids: Vec<String> = (0..statuses.len())
        .map(|index| format!("orb-{index:06}"))
        .collect();
    let mut tasks: Vec<Task> = ids
        .iter()
        .cloned()
        .zip(statuses.iter().copied())
        .map(|(id, status)| task(id, status, Vec::new()))
        .collect();

    let mut seen = HashSet::new();
    for &(left, right) in edges {
        if left == right {
            continue;
        }
        let (from, to) = if left > right {
            (left, right)
        } else {
            (right, left)
        };
        if !seen.insert((from, to)) {
            continue;
        }
        tasks[from].front.links.push(Link::dependency(&ids[to]));
    }

    (
        Graph::from_tasks_with_workflow(tasks, Vec::new(), Workflow::standard()),
        ids,
    )
}

proptest! {
    #[test]
    fn readiness_matches_the_definition(
        edges in prop::collection::vec((0usize..12, 0usize..12), 0..40),
        statuses in prop::collection::vec(
            prop_oneof![
                Just("ready"),
                Just("todo"),
                Just("in_progress"),
                Just("done"),
                Just("cancelled"),
            ],
            12
        ),
    ) {
        let (graph, _) = build_dag(&edges, &statuses);
        prop_assert!(graph.find_cycle().is_none());
        let workflow = &graph.workflow;

        for task in graph.tasks.values() {
            let expected: Vec<String> = task
                .blocking_links()
                .filter_map(|link| link.target_task())
                .filter(|dependency| {
                    graph
                        .tasks
                        .get(*dependency)
                        .map(|dependency| !workflow.is_terminal(&dependency.front.status))
                        .unwrap_or(true)
                })
                .map(str::to_string)
                .collect();

            prop_assert_eq!(graph.blocking_ids(task), expected);
            prop_assert_eq!(
                graph.is_ready(task),
                workflow.is_ready_status(&task.front.status)
                    && task.front.assignee.is_none()
                    && graph.blocking_ids(task).is_empty()
            );
        }

        for task in graph.ready_tasks() {
            prop_assert!(graph.workflow.is_ready_status(&task.front.status));
            prop_assert!(task.front.assignee.is_none());
            let all_dependencies_terminal = task
                .blocking_links()
                .filter_map(|link| link.target_task())
                .all(|dependency| {
                    graph
                        .tasks
                        .get(dependency)
                        .is_some_and(|dependency| {
                            graph.workflow.is_terminal(&dependency.front.status)
                        })
                });
            prop_assert!(all_dependencies_terminal);
        }
    }

    #[test]
    fn reverse_edges_create_cycles(
        edges in prop::collection::vec((0usize..12, 0usize..12), 1..40),
    ) {
        let statuses = vec!["ready"; 12];
        let (graph, ids) = build_dag(&edges, &statuses);

        let first = edges
            .iter()
            .copied()
            .find(|(left, right)| left != right)
            .map(|(left, right)| if left > right { (left, right) } else { (right, left) });

        if let Some((from, to)) = first {
            let cycle = graph.would_create_cycle(&ids[to], &ids[from]);
            prop_assert!(cycle.is_some(), "expected a cycle for {} -> {}", ids[to], ids[from]);
            let cycle = cycle.unwrap();
            prop_assert_eq!(cycle.first().unwrap(), &ids[to]);
            prop_assert_eq!(cycle.last().unwrap(), &ids[to]);
        }
    }
}
