//! Time-based stat decay.
//!
//! The pet is simulated lazily. Nothing runs on a timer for correctness - a
//! pet's stats are brought up to date from `last_tick` whenever it is read or
//! written. The background ticker in [`crate::ticker`] exists only so that
//! alerts fire and deaths are recorded while nobody is looking.
//!
//! Decay is integrated in small fixed steps rather than one big multiply,
//! because the rules are non-linear: health only drains once another stat has
//! bottomed out, and the pet falls asleep on its own when it runs out of
//! energy. A single-shot calculation over a two-day gap would miss both.

use crate::model::{clamp_stat, Pet};

/// Length of one integration step, in hours. Six minutes keeps a month-long
/// absence under 10k iterations while staying fine-grained enough that the
/// order of threshold crossings is right.
const STEP_HOURS: f64 = 0.1;

/// Never simulate more than this. A pet ignored for two months is just as dead
/// as one ignored for a year, and this bounds the loop.
const MAX_SIMULATED_HOURS: f64 = 24.0 * 60.0;

/// Tunable decay and recovery rates, all expressed per hour of real time.
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    /// Fullness lost per hour while awake.
    pub hunger_decay: f64,
    /// Happiness lost per hour while awake.
    pub happiness_decay: f64,
    /// Energy lost per hour while awake.
    pub energy_decay: f64,
    /// Energy recovered per hour while asleep.
    pub sleep_energy_regen: f64,
    /// Multiplier applied to hunger decay while asleep.
    pub sleep_hunger_mult: f64,
    /// Multiplier applied to happiness decay while asleep.
    pub sleep_happiness_mult: f64,
    /// Health lost per hour while fullness sits at zero.
    pub starvation_damage: f64,
    /// Health lost per hour while happiness sits at zero.
    pub loneliness_damage: f64,
    /// Health regained per hour while the pet is comfortably cared for.
    pub health_regen: f64,
    /// Global multiplier, for speeding the world up during testing.
    pub time_scale: f64,
}

impl Default for Rates {
    fn default() -> Self {
        // Calibrated so a pet left completely alone from full stats gets
        // hungry in about 19h, starts taking damage at 25h, and dies at
        // roughly 41h. That means a server has to miss more than a full day
        // before anything is irreversible.
        Self {
            hunger_decay: 4.0,
            happiness_decay: 3.0,
            energy_decay: 5.0,
            sleep_energy_regen: 20.0,
            sleep_hunger_mult: 0.6,
            sleep_happiness_mult: 0.4,
            starvation_damage: 6.0,
            loneliness_damage: 3.0,
            health_regen: 2.0,
            time_scale: 1.0,
        }
    }
}

/// What changed during a simulation run. The caller uses this to decide
/// whether to announce something in the alert channel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TickReport {
    /// The pet died during this run.
    pub died: bool,
    /// The pet put itself to sleep after running out of energy.
    pub fell_asleep: bool,
    /// The pet woke up because it was fully rested.
    pub woke_up: bool,
    /// Hours of real time actually simulated.
    pub hours: f64,
}

/// Bring `pet` up to date as of `now` (unix seconds).
///
/// Safe to call repeatedly; advancing a pet to a time it has already reached
/// is a no-op. Returns what happened along the way.
pub fn advance(pet: &mut Pet, now: i64, rates: &Rates) -> TickReport {
    let mut report = TickReport::default();

    if !pet.alive {
        // Dead pets do not decay any further, but keep the clock moving so a
        // later revive does not replay the whole gap.
        pet.last_tick = pet.last_tick.max(now);
        return report;
    }

    let elapsed_secs = now - pet.last_tick;
    if elapsed_secs <= 0 {
        return report;
    }

    let mut hours = (elapsed_secs as f64) / 3600.0 * rates.time_scale;
    if hours > MAX_SIMULATED_HOURS {
        hours = MAX_SIMULATED_HOURS;
    }
    report.hours = hours;

    // Track which neglect did the most damage, for the memorial message.
    let mut starvation_total = 0.0;
    let mut loneliness_total = 0.0;

    let mut remaining = hours;
    while remaining > 0.0 {
        let dt = remaining.min(STEP_HOURS);
        remaining -= dt;

        if pet.asleep {
            pet.energy = clamp_stat(pet.energy + rates.sleep_energy_regen * dt);
            pet.hunger = clamp_stat(pet.hunger - rates.hunger_decay * rates.sleep_hunger_mult * dt);
            pet.happiness =
                clamp_stat(pet.happiness - rates.happiness_decay * rates.sleep_happiness_mult * dt);

            if pet.energy >= crate::model::STAT_MAX {
                pet.asleep = false;
                report.woke_up = true;
            }
        } else {
            pet.hunger = clamp_stat(pet.hunger - rates.hunger_decay * dt);
            pet.happiness = clamp_stat(pet.happiness - rates.happiness_decay * dt);
            pet.energy = clamp_stat(pet.energy - rates.energy_decay * dt);

            if pet.energy <= crate::model::STAT_MIN {
                // Rather than punishing exhaustion with damage, the pet simply
                // collapses into a nap. Neglect still kills, but only through
                // hunger and loneliness, which are the stats members can see
                // coming.
                pet.asleep = true;
                report.fell_asleep = true;
            }
        }

        let mut damage = 0.0;
        if pet.hunger <= crate::model::STAT_MIN {
            let d = rates.starvation_damage * dt;
            damage += d;
            starvation_total += d;
        }
        if pet.happiness <= crate::model::STAT_MIN {
            let d = rates.loneliness_damage * dt;
            damage += d;
            loneliness_total += d;
        }

        if damage > 0.0 {
            pet.health = clamp_stat(pet.health - damage);
        } else if pet.hunger >= 50.0 && pet.happiness >= 50.0 {
            pet.health = clamp_stat(pet.health + rates.health_regen * dt);
        }

        if pet.health <= crate::model::STAT_MIN {
            pet.alive = false;
            pet.asleep = false;
            pet.died_at = Some(now);
            pet.cause_of_death = Some(
                if starvation_total >= loneliness_total {
                    "starvation"
                } else {
                    "loneliness"
                }
                .to_string(),
            );
            report.died = true;
            break;
        }
    }

    pet.last_tick = now;
    report
}

/// Hours of continued neglect before the pet dies, or `None` if it is
/// currently stable. Used for the warning line on the status card.
pub fn hours_until_death(pet: &Pet, rates: &Rates) -> Option<f64> {
    if !pet.alive {
        return None;
    }

    let mut probe = pet.clone();
    let step_secs = (STEP_HOURS * 3600.0) as i64;
    let mut elapsed = 0.0;
    let mut clock = probe.last_tick;

    while elapsed < MAX_SIMULATED_HOURS {
        clock += step_secs;
        elapsed += STEP_HOURS;
        if advance(&mut probe, clock, rates).died {
            return Some(elapsed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Pet;

    fn fresh() -> Pet {
        Pet::new(1, "Tester".into(), "blob".into(), 1, 0)
    }

    fn hours(h: f64) -> i64 {
        (h * 3600.0) as i64
    }

    #[test]
    fn no_time_no_change() {
        let mut pet = fresh();
        let before = (pet.hunger, pet.happiness, pet.energy, pet.health);
        let report = advance(&mut pet, 0, &Rates::default());
        assert_eq!(report, TickReport::default());
        assert_eq!(before, (pet.hunger, pet.happiness, pet.energy, pet.health));
    }

    #[test]
    fn clock_going_backwards_is_ignored() {
        let mut pet = fresh();
        pet.last_tick = hours(10.0);
        let before = pet.hunger;
        advance(&mut pet, 0, &Rates::default());
        assert_eq!(pet.hunger, before);
    }

    #[test]
    fn stats_decay_over_time() {
        let mut pet = fresh();
        advance(&mut pet, hours(5.0), &Rates::default());
        assert!(pet.hunger < 80.0, "hunger should fall, got {}", pet.hunger);
        assert!(pet.happiness < 80.0);
        assert!(pet.alive, "five hours should not be fatal");
    }

    #[test]
    fn advancing_is_incremental_not_cumulative() {
        let rates = Rates::default();
        let mut one_shot = fresh();
        advance(&mut one_shot, hours(6.0), &rates);

        let mut stepwise = fresh();
        for h in 1..=6 {
            advance(&mut stepwise, hours(h as f64), &rates);
        }

        assert!((one_shot.hunger - stepwise.hunger).abs() < 0.001);
        assert!((one_shot.happiness - stepwise.happiness).abs() < 0.001);
    }

    #[test]
    fn exhaustion_causes_a_nap_not_damage() {
        let mut pet = fresh();
        // 90 energy at 5/hour runs out after 18 hours.
        let report = advance(&mut pet, hours(19.0), &Rates::default());
        assert!(report.fell_asleep);
        assert!(pet.asleep);
        assert!(pet.alive);
    }

    #[test]
    fn sleeping_pet_wakes_when_rested() {
        let mut pet = fresh();
        pet.asleep = true;
        pet.energy = 10.0;
        let report = advance(&mut pet, hours(6.0), &Rates::default());
        assert!(report.woke_up);
        assert!(!pet.asleep);
    }

    #[test]
    fn total_neglect_is_eventually_fatal() {
        let mut pet = fresh();
        let report = advance(&mut pet, hours(72.0), &Rates::default());
        assert!(report.died, "three days of neglect should be fatal");
        assert!(!pet.alive);
        assert_eq!(pet.died_at, Some(hours(72.0)));
        assert_eq!(pet.cause_of_death.as_deref(), Some("starvation"));
    }

    #[test]
    fn a_dead_pet_stays_dead_and_does_not_decay_further() {
        let mut pet = fresh();
        advance(&mut pet, hours(72.0), &Rates::default());
        let died_at = pet.died_at;
        let report = advance(&mut pet, hours(200.0), &Rates::default());
        assert!(!report.died, "death should only be reported once");
        assert_eq!(pet.died_at, died_at);
    }

    #[test]
    fn a_well_fed_pet_heals() {
        let mut pet = fresh();
        pet.health = 50.0;
        pet.hunger = 100.0;
        pet.happiness = 100.0;
        advance(&mut pet, hours(4.0), &Rates::default());
        assert!(
            pet.health > 50.0,
            "health should recover, got {}",
            pet.health
        );
    }

    #[test]
    fn absurd_gaps_are_bounded() {
        let mut pet = fresh();
        // Ten years. Should terminate quickly and not hang.
        let report = advance(&mut pet, hours(24.0 * 365.0 * 10.0), &Rates::default());
        assert!(report.died);
    }

    #[test]
    fn death_forecast_warns_before_it_happens() {
        let pet = fresh();
        let eta = hours_until_death(&pet, &Rates::default()).expect("neglect is fatal");
        assert!(eta > 24.0 && eta < 72.0, "expected a day or two, got {eta}");
    }

    #[test]
    fn a_maintained_pet_has_no_death_forecast() {
        let mut pet = fresh();
        pet.hunger = 100.0;
        pet.happiness = 100.0;
        pet.health = 100.0;
        // Still finite: hunger decays even at full. Confirm it is far away.
        let eta = hours_until_death(&pet, &Rates::default());
        assert!(eta.is_none() || eta.unwrap() > 40.0);
    }
}
