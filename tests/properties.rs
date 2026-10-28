use chrono::{TimeZone, Utc};
use orbit_tracker::graph::Graph;
use orbit_tracker::task::{Status, Task, TaskFrontmatter, TaskType};
use proptest::prelude::*;
use std::collections::{BTreeMap, HashSet};

fn task(id: String, status: Status, dependencies: Vec<String>) -> Task {
    let now = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
    Task {
        id,
        front: TaskFrontmatter {
            title: "property task".to_string(),
            status,
            task_type: TaskType::Task,
            priority: 2,
            labels: Vec::new(),
            depends_on: dependencies,
            parent: None,
            waiting_on: None,
            area: None,
            verification: None,
            claim: None,
            stray_claim_times: None,
            created_at: now,
            updated_at: now,
            extra: BTreeMap::new(),
        },
        body: String::new(),
    }
}

fn build_dag(edges: &[(usize, usize)], statuses: &[Status]) -> (Graph, Vec<String>) {
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
        let dependency = ids[to].clone();
        tasks[from].front.depends_on.push(dependency);
    }

    (Graph::from_tasks(tasks), ids)
}

proptest! {
    #[test]
    fn readiness_matches_the_definition(
        edges in prop::collection::vec((0usize..12, 0usize..12), 0..40),
        statuses in prop::collection::vec(
            prop_oneof![
                Just(Status::Todo),
                Just(Status::InProgress),
                Just(Status::Done),
                Just(Status::Cancelled),
            ],
            12
        ),
    ) {
        let (graph, _) = build_dag(&edges, &statuses);
        prop_assert!(graph.find_cycle().is_none());

        for task in graph.tasks.values() {
            let expected: Vec<String> = task
                .front
                .depends_on
                .iter()
                .filter(|dependency| {
                    graph
                        .tasks
                        .get(*dependency)
                        .map(|dependency| !dependency.front.status.is_terminal())
                        .unwrap_or(true)
                })
                .cloned()
                .collect();

            prop_assert_eq!(graph.blocking_ids(task), expected);
            prop_assert_eq!(
                graph.is_ready(task),
                task.front.status == Status::Todo
                    && task.front.claim.is_none()
                    && task.front.waiting_on.is_none()
                    && graph.blocking_ids(task).is_empty()
            );
        }

        for task in graph.ready_tasks() {
            prop_assert_eq!(task.front.status, Status::Todo);
            prop_assert!(task.front.claim.is_none());
            let all_dependencies_terminal = task.front.depends_on.iter().all(|dependency| {
                graph
                    .tasks
                    .get(dependency)
                    .is_some_and(|dependency| dependency.front.status.is_terminal())
            });
            prop_assert!(all_dependencies_terminal);
        }
    }

    #[test]
    fn reverse_edges_create_cycles(
        edges in prop::collection::vec((0usize..12, 0usize..12), 1..40),
    ) {
        let statuses = vec![Status::Todo; 12];
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
