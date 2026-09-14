use cordis_loader::LoadOutcome;
use cordis_loader::outcome::EntryOutcome;

fn old(entry: &EntryOutcome, outcome: &LoadOutcome) {
    if let EntryOutcome::Spawned { fork, .. } = entry {
        let _ = fork;
    }
    let _ = outcome.forks();
}

fn main() {}
