use std::cell::RefCell;
use std::rc::Rc;

use super::*;

const IDLE: Instance = Instance::None;

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn facts(flag: Flag, this: &str, installed: Option<&str>, this_is_installed: bool, running: Instance) -> Facts {
    Facts { policy: Policy::SelfManaged, flag, this_version: v(this), this_is_installed, installed: installed.map(v), running }
}

#[test]
fn versions_parse_and_order() {
    assert_eq!(v("0.2.0"), Version { major: 0, minor: 2, patch: 0 });
    assert_eq!(v("1.2"), Version { major: 1, minor: 2, patch: 0 });
    assert_eq!(v("0.3.1-dev"), Version { major: 0, minor: 3, patch: 1 });
    assert!(v("0.10.0") > v("0.9.9"));
    assert!(v("1.0.0") > v("0.99.99"));
    assert_eq!(Version::parse("x.1"), None);
    assert_eq!(v("0.2.0").to_string(), "0.2.0");
}

#[test]
fn flags_from_args() {
    assert_eq!(Flag::from_args(["meltalarm.exe"]), Flag::None);
    assert_eq!(Flag::from_args(["x", "--portable"]), Flag::Portable);
    assert_eq!(Flag::from_args(["x", "--install"]), Flag::Install);
    assert_eq!(Flag::from_args(["x", "--portable", "--uninstall"]), Flag::Uninstall);
}

/// Spec §4.4, in order.
#[test]
fn launch_table_self_managed() {
    use Instance::{OtherFile, SameFile};
    use Launch::*;
    let cases = [
        // 1. the installed copy starts normally, or hands off to a running monitor
        (facts(Flag::None, "0.2.0", Some("0.2.0"), true, IDLE), Monitor { portable: false }),
        (facts(Flag::None, "0.2.0", Some("0.2.0"), true, SameFile), HandOff),
        (facts(Flag::None, "0.2.0", Some("0.2.0"), true, OtherFile), HandOff),
        // 2. this same file is already running
        (facts(Flag::None, "0.2.0", None, false, SameFile), HandOff),
        // 3. nothing installed → offer install (a running portable copy of another file is stopped by it)
        (facts(Flag::None, "0.2.0", None, false, IDLE), OfferInstall),
        (facts(Flag::None, "0.2.0", None, false, OtherFile), OfferInstall),
        // 4. another copy installed → compare versions
        (facts(Flag::None, "0.3.0", Some("0.2.0"), false, OtherFile), OfferUpdate { from: v("0.2.0"), to: v("0.3.0") }),
        (facts(Flag::None, "0.1.0", Some("0.2.0"), false, IDLE), OfferReplace { from: v("0.2.0"), to: v("0.1.0") }),
        (facts(Flag::None, "0.2.0", Some("0.2.0"), false, OtherFile), StartInstalled),
        (facts(Flag::None, "0.2.0", Some("0.2.0"), false, IDLE), StartInstalled),
        // an installed copy whose version can't be read counts as oldest
        (facts(Flag::None, "0.2.0", Some("0.0.0"), false, IDLE), OfferUpdate { from: v("0.0.0"), to: v("0.2.0") }),
    ];
    for (f, want) in cases {
        assert_eq!(decide(&f), want, "{f:?}");
    }
}

#[test]
fn flags_override_the_table() {
    use Instance::{OtherFile, SameFile};
    use Launch::*;
    // --portable: never an offer; never a second monitor
    assert_eq!(decide(&facts(Flag::Portable, "0.3.0", Some("0.2.0"), false, IDLE)), Monitor { portable: true });
    assert_eq!(decide(&facts(Flag::Portable, "0.3.0", None, false, OtherFile)), HandOff);
    // --install from a running portable copy (it is the "same file" running)
    assert_eq!(decide(&facts(Flag::Install, "0.2.0", None, false, SameFile)), OfferInstall);
    assert_eq!(decide(&facts(Flag::Install, "0.2.0", Some("0.2.0"), true, SameFile)), HandOff);
    // --uninstall works from the installed copy even while it is running
    assert_eq!(decide(&facts(Flag::Uninstall, "0.2.0", Some("0.2.0"), true, SameFile)), Uninstall);
    assert_eq!(decide(&facts(Flag::Uninstall, "0.2.0", Some("0.2.0"), false, IDLE)), Uninstall);
    assert_eq!(decide(&facts(Flag::Uninstall, "0.2.0", None, false, IDLE)), NotInstalled);
}

#[test]
fn run_without_installing_never_starts_a_second_monitor() {
    assert_eq!(facts(Flag::None, "0.2.0", None, false, Instance::None).without_installing(), Launch::Monitor { portable: true });
    assert_eq!(facts(Flag::None, "0.2.0", None, false, Instance::OtherFile).without_installing(), Launch::HandOff);
}

#[test]
fn package_managed_never_offers_anything() {
    for flag in [Flag::None, Flag::Install, Flag::Uninstall] {
        for installed in [None, Some("0.1.0"), Some("0.9.0")] {
            let mut f = facts(flag, "0.2.0", installed, false, Instance::None);
            f.policy = Policy::PackageManaged;
            assert_eq!(decide(&f), Launch::Monitor { portable: false });
            f.running = Instance::SameFile;
            assert_eq!(decide(&f), Launch::HandOff);
        }
    }
}

/// Records apply/undo order; fails at `fail`.
struct Fake {
    n: usize,
    fail: bool,
    journal: Rc<RefCell<Vec<String>>>,
}

impl Step for Fake {
    fn name(&self) -> String {
        format!("step {}", self.n)
    }
    fn apply(&mut self) -> Result<(), String> {
        self.journal.borrow_mut().push(format!("apply {}", self.n));
        if self.fail { Err("boom".into()) } else { Ok(()) }
    }
    fn undo(&mut self) {
        self.journal.borrow_mut().push(format!("undo {}", self.n));
    }
}

type Journal = Rc<RefCell<Vec<String>>>;

fn plan(len: usize, fail_at: Option<usize>) -> (Vec<Box<dyn Step>>, Journal) {
    let journal = Rc::new(RefCell::new(vec![]));
    let steps = (0..len).map(|n| Box::new(Fake { n, fail: Some(n) == fail_at, journal: journal.clone() }) as Box<dyn Step>).collect();
    (steps, journal)
}

#[test]
fn atomic_run_undoes_done_steps_in_reverse_at_every_failure_position() {
    let (mut steps, journal) = plan(3, None);
    assert_eq!(run_atomic(&mut steps), Ok(()));
    assert_eq!(*journal.borrow(), ["apply 0", "apply 1", "apply 2"]);

    for fail_at in 0..3 {
        let (mut steps, journal) = plan(3, Some(fail_at));
        let err = run_atomic(&mut steps).unwrap_err();
        assert_eq!(err, Failed { step: format!("step {fail_at}"), error: "boom".into() });
        let mut want: Vec<String> = (0..=fail_at).map(|n| format!("apply {n}")).collect();
        want.extend((0..fail_at).rev().map(|n| format!("undo {n}")));
        assert_eq!(*journal.borrow(), want, "fail at {fail_at}");
    }
}

#[test]
fn best_effort_run_continues_and_reports() {
    let (mut steps, journal) = plan(3, Some(1));
    let failed = run_best_effort(&mut steps);
    assert_eq!(failed, [Failed { step: "step 1".into(), error: "boom".into() }]);
    assert_eq!(*journal.borrow(), ["apply 0", "apply 1", "apply 2"]);
}
