use cordis_loader::{EntryId, LoadOutcome};
use cordis_loader::outcome::{EntryOutcome, LoaderFailure};

fn inspect(outcome: &LoadOutcome, id: &EntryId) {
    let _: &[EntryOutcome] = outcome.entries();
    let _: &EntryId = outcome.entries()[0].id();
    let _: Option<&EntryOutcome> = outcome.entry(id);
    let _: bool = outcome.is_ok();
    let _forks = outcome.forks();
}

fn failure_is_specialist(failure: &LoaderFailure) {
    let _: &dyn std::error::Error = failure;
}

fn main() {
    let _ = inspect;
    let _ = failure_is_specialist;
}
