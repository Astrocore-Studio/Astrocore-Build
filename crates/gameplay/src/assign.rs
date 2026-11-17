//! Раздача должностей на старте раунда (`ServerStationJobsSystem.AssignJobs` SS14).
//!
//! Фаза 1 — обязательные места: должности с `start > 0` по убыванию веса (при равенстве по
//! идентификатору); на каждое место берётся игрок с самым высоким приоритетом именно на эту
//! должность (высокий → средний → низкий), среди равных случайный. Фаза 2 — остальные игроки в
//! случайном порядке: каждому достаётся его самая желанная из ещё свободных должностей (среди
//! равных по приоритету случайная). Оставшиеся без должности получают переполняющую (без
//! предела мест, как «пассажир»), если их профиль это разрешает, иначе остаются в лобби.

use std::collections::BTreeMap;

use astrocore_core::NetworkEntityId;

use crate::character::{Priority, Unavailable};
use crate::disposal::Rng;

/// Должность для раздачи.
#[derive(Debug, Clone)]
pub struct Opening {
    pub id: String,
    /// Мест на старте раунда обязательно (`start`); меньше нуля или ноль — не обязательные.
    pub start: i32,
    /// Всего мест; меньше нуля — без предела.
    pub max: i32,
    pub weight: i32,
}

/// Игрок со своими предпочтениями.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub who: NetworkEntityId,
    pub priorities: BTreeMap<String, Priority>,
    pub unavailable: Unavailable,
}

fn pick<T: Clone>(items: &[T], rng: &mut Rng) -> T {
    let i = ((rng.next_f32() * items.len() as f32) as usize).min(items.len() - 1);
    items[i].clone()
}

fn shuffle<T>(items: &mut [T], rng: &mut Rng) {
    for i in (1..items.len()).rev() {
        let j = ((rng.next_f32() * (i + 1) as f32) as usize).min(i);
        items.swap(i, j);
    }
}

/// Кому какая должность: `None` — остаётся в лобби. Порядок результата — порядок `players`.
pub fn assign(
    openings: &[Opening],
    players: &[Candidate],
    rng: &mut Rng,
) -> Vec<(NetworkEntityId, Option<String>)> {
    // Сколько мест осталось (`None` — без предела).
    let mut left: BTreeMap<&str, Option<i32>> = openings
        .iter()
        .map(|o| (o.id.as_str(), (o.max >= 0).then_some(o.max)))
        .collect();
    let mut result: BTreeMap<NetworkEntityId, String> = BTreeMap::new();
    let free = |left: &BTreeMap<&str, Option<i32>>, job: &str| {
        left.get(job).is_some_and(|l| l.is_none_or(|n| n > 0))
    };

    // Фаза 1: обязательные места.
    let mut required: Vec<&Opening> = openings.iter().filter(|o| o.start > 0).collect();
    required.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.id.cmp(&b.id)));
    for job in required {
        for _ in 0..job.start {
            if !free(&left, &job.id) {
                break;
            }
            let mut chosen = None;
            for priority in [Priority::High, Priority::Medium, Priority::Low] {
                let candidates: Vec<NetworkEntityId> = players
                    .iter()
                    .filter(|p| !result.contains_key(&p.who))
                    .filter(|p| p.priorities.get(&job.id) == Some(&priority))
                    .map(|p| p.who)
                    .collect();
                if !candidates.is_empty() {
                    chosen = Some(pick(&candidates, rng));
                    break;
                }
            }
            let Some(who) = chosen else { break };
            result.insert(who, job.id.clone());
            if let Some(Some(n)) = left.get_mut(job.id.as_str()) {
                *n -= 1;
            }
        }
    }

    // Фаза 2: каждому его лучшая свободная должность.
    let mut rest: Vec<&Candidate> = players
        .iter()
        .filter(|p| !result.contains_key(&p.who))
        .collect();
    shuffle(&mut rest, rng);
    for p in rest {
        for priority in [Priority::High, Priority::Medium, Priority::Low] {
            let jobs: Vec<&str> = p
                .priorities
                .iter()
                .filter(|(_, pr)| **pr == priority)
                .map(|(id, _)| id.as_str())
                .filter(|id| free(&left, id))
                .collect();
            if jobs.is_empty() {
                continue;
            }
            let job = pick(&jobs, rng);
            result.insert(p.who, job.to_string());
            if let Some(Some(n)) = left.get_mut(job) {
                *n -= 1;
            }
            break;
        }
    }

    // Переполнение: должности без предела мест.
    let overflow: Vec<&str> = openings
        .iter()
        .filter(|o| o.max < 0)
        .map(|o| o.id.as_str())
        .collect();
    players
        .iter()
        .map(|p| {
            let job = result.get(&p.who).cloned().or_else(|| {
                (p.unavailable == Unavailable::SpawnAsOverflow && !overflow.is_empty())
                    .then(|| pick(&overflow, rng).to_string())
            });
            (p.who, job)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(id: &str, start: i32, max: i32, weight: i32) -> Opening {
        Opening {
            id: id.into(),
            start,
            max,
            weight,
        }
    }

    fn player(n: u32, prefs: &[(&str, Priority)], unavailable: Unavailable) -> Candidate {
        Candidate {
            who: NetworkEntityId(n),
            priorities: prefs.iter().map(|(j, p)| ((*j).to_string(), *p)).collect(),
            unavailable,
        }
    }

    fn jobs() -> Vec<Opening> {
        vec![
            open("Captain", 1, 1, 20),
            open("Chef", 0, 1, 0),
            open("Engineer", 2, 4, 0),
            open("Passenger", -1, -1, 0),
        ]
    }

    fn job_of(r: &[(NetworkEntityId, Option<String>)], n: u32) -> Option<String> {
        r.iter()
            .find(|(w, _)| w.0 == n)
            .and_then(|(_, j)| j.clone())
    }

    use Priority::{High, Low, Medium};
    use Unavailable::{SpawnAsOverflow, StayInLobby};

    #[test]
    fn a_high_preference_beats_a_lower_one_for_the_required_seat() {
        let players = [
            player(1, &[("Captain", Low)], SpawnAsOverflow),
            player(2, &[("Captain", High)], SpawnAsOverflow),
            player(3, &[("Captain", Medium)], SpawnAsOverflow),
        ];
        for seed in 1..30 {
            let r = assign(&jobs(), &players, &mut Rng(seed * 7919));
            assert_eq!(job_of(&r, 2).as_deref(), Some("Captain"), "seed {seed}");
            assert_eq!(job_of(&r, 1).as_deref(), Some("Passenger"));
            assert_eq!(job_of(&r, 3).as_deref(), Some("Passenger"));
        }
    }

    #[test]
    fn seats_are_never_over_filled_and_every_player_gets_something() {
        let players: Vec<Candidate> = (1..=10)
            .map(|n| player(n, &[("Engineer", High), ("Chef", Medium)], SpawnAsOverflow))
            .collect();
        let r = assign(&jobs(), &players, &mut Rng(42));
        let count = |j: &str| r.iter().filter(|(_, x)| x.as_deref() == Some(j)).count();
        assert_eq!(count("Engineer"), 4, "предел мест");
        assert_eq!(count("Chef"), 1);
        assert_eq!(count("Passenger"), 5, "остальные — переполнение");
        assert_eq!(r.len(), 10);
    }

    #[test]
    fn stay_in_lobby_keeps_unlucky_players_out() {
        let players = [
            player(1, &[("Captain", High)], StayInLobby),
            player(2, &[("Captain", High)], StayInLobby),
        ];
        let r = assign(&jobs(), &players, &mut Rng(5));
        let got: Vec<_> = r.iter().filter_map(|(_, j)| j.clone()).collect();
        assert_eq!(got, vec!["Captain"], "капитан один, второй остался в лобби");
        assert_eq!(r.iter().filter(|(_, j)| j.is_none()).count(), 1);
    }

    #[test]
    fn required_seats_are_filled_by_weight_and_unwanted_ones_stay_empty() {
        // Одно желание двух должностей: командование (вес 20) обязательнее инженера.
        let players = [player(
            1,
            &[("Captain", Medium), ("Engineer", High)],
            SpawnAsOverflow,
        )];
        let r = assign(&jobs(), &players, &mut Rng(9));
        assert_eq!(
            job_of(&r, 1).as_deref(),
            Some("Captain"),
            "обязательное место тяжелее по весу"
        );
        // Никто не хочет инженера: обязательные места остаются пустыми, а не навязываются.
        let nobody = [player(1, &[("Chef", High)], SpawnAsOverflow)];
        let r = assign(&jobs(), &nobody, &mut Rng(9));
        assert_eq!(job_of(&r, 1).as_deref(), Some("Chef"));
    }

    #[test]
    fn without_an_overflow_job_players_with_no_match_wait() {
        let only_seats = [open("Captain", 1, 1, 20)];
        let r = assign(&only_seats, &[player(1, &[], SpawnAsOverflow)], &mut Rng(1));
        assert_eq!(job_of(&r, 1), None);
    }

    #[test]
    fn the_same_seed_gives_the_same_result() {
        let players: Vec<Candidate> = (1..=8)
            .map(|n| {
                player(
                    n,
                    &[("Engineer", Medium), ("Chef", Medium)],
                    SpawnAsOverflow,
                )
            })
            .collect();
        let a = assign(&jobs(), &players, &mut Rng(77));
        let b = assign(&jobs(), &players, &mut Rng(77));
        assert_eq!(a, b);
    }
}
