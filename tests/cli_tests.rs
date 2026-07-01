//! Integration tests for the `cli` module — exercise the clap derive
//! parser through its public API using `try_parse_from`. No mocks.

use clap::error::ErrorKind;
use clap::Parser as _;

use teehee::cli::{Cli, Command};

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    let iter = std::iter::once("teehee").chain(args.iter().copied());
    Cli::try_parse_from(iter)
}

#[test]
fn teehee_flag_parses() {
    let args = parse(&["--help"]);
    // -h/--help always reports an error (since clap prints help to stderr
    // and exits). We're only checking the parser recognises the flag.
    assert!(args.is_err(), "`--help` should signal an error");
    let err = args.err().unwrap();
    assert!(
        err.kind() == ErrorKind::DisplayHelp || err.kind() == ErrorKind::DisplayVersion,
        "expected help/version kind, got {:?}",
        err.kind()
    );
}

#[test]
fn devices_subcommand_parses_with_no_flags() {
    let cli = parse(&["devices"]).expect("`teehee devices` with no args is valid");
    assert!(matches!(cli.command, Command::Devices));
}

#[test]
fn send_defaults_are_pinned() {
    let cli = parse(&["send", "192.168.0.10"]).expect("`send` with host only is valid");
    let Command::Send(args) = cli.command else {
        panic!("expected Send variant");
    };
    // `host` is now Option<String> (since --host is the canonical form
    // per the PRD); the positional form remains accepted via clap but
    // is optional so --host can be used instead.
    assert_eq!(args.host.as_deref(), Some("192.168.0.10"));
    assert!(args.host_flag.is_none());
    assert_eq!(args.port, 5000);
    assert_eq!(args.chunk_ms, 20);
    assert!(!args.sine);
    assert!(!args.stats);
}

#[test]
fn send_with_explicit_port_sine_and_stats() {
    // Note: avoid the `host:port` form here so we don't trip the
    // "ambiguous port" collision path that the dedicated test covers.
    let cli = parse(&["send", "10.0.0.5", "--port", "6000", "--sine", "--stats"])
        .expect("send with flags is valid");
    let Command::Send(args) = cli.command else {
        panic!("expected Send variant");
    };
    assert_eq!(args.port, 6000);
    assert!(args.sine);
    assert!(args.stats);
    // validate() should accept: --port 6000 with no embedded port is
    // unambiguous, so the resolved target is host=10.0.0.5, port=6000.
    let t = args.validate().expect("validate ok");
    assert_eq!(t.host(), Some("10.0.0.5"));
    assert_eq!(t.port(), Some(6000));
}

#[test]
fn send_without_host_is_rejected() {
    // clap now treats positional HOST as optional (it's Option<String>),
    // so the missing-host error surfaces from validate() rather than at
    // parse time. Either rejection point is correct; we assert the
    // user-facing "must require exactly one of positional/--host/--port"
    // contract surfaces an error.
    let cli = parse(&["send"]).expect("parse ok (validate is the gate)");
    let Command::Send(args) = cli.command else {
        panic!("expected Send variant");
    };
    let err = args
        .validate()
        .expect_err("send without positional HOST or --host must error at validate()");
    assert!(
        err.contains("destination host required") || err.contains("--mdns"),
        "expected required-host message (mentions --mdns too), got {err:?}"
    );
}

#[test]
fn send_with_ambiguous_port_is_rejected() {
    // Regression for the review-flagged bug: `send <host:port> --port <p>`
    // produced `<host:port>:<p>` on the wire. The new validate() must
    // surface this as "ambiguous port" rather than silently doubling.
    let cli = parse(&["send", "10.0.0.5:6000", "--port", "6001"])
        .expect("clap parses (validate is the gate)");
    let Command::Send(args) = cli.command else {
        panic!("expected Send variant");
    };
    let err = args
        .validate()
        .expect_err("ambiguous port must error at validate()");
    assert!(
        err.contains("ambiguous port"),
        "expected ambiguous-port message, got {err:?}"
    );
}

#[test]
fn send_with_host_flag_parses() {
    // The PRD-canonical command form: `teehee send --host <ip>`.
    // Pre-this-slice this very command rejected as "unexpected argument
    // '--host'"; the regression test pins the fix.
    let cli = parse(&["send", "--host", "10.0.0.5", "--sine"]).expect("`--host` flag must parse");
    let Command::Send(args) = cli.command else {
        panic!("expected Send variant");
    };
    assert!(args.host.is_none());
    assert_eq!(args.host_flag.as_deref(), Some("10.0.0.5"));
    let t = args.validate().expect("validate ok");
    assert_eq!(t.host(), Some("10.0.0.5"));
    assert_eq!(t.port(), Some(5000));
}

#[test]
fn send_with_host_flag_and_embedded_port() {
    let cli = parse(&["send", "--host", "10.0.0.5:6000", "--sine"])
        .expect("clap parses --host <ip:port>");
    let Command::Send(args) = cli.command else {
        panic!("expected Send variant");
    };
    let t = args.validate().expect("validate ok");
    assert_eq!(t.host(), Some("10.0.0.5"));
    assert_eq!(t.port(), Some(6000));
}

#[test]
fn send_with_both_positional_and_host_flag_is_rejected() {
    // clap-level rejection: host_flag has conflicts_with = "host".
    let result = parse(&["send", "127.0.0.1", "--host", "10.0.0.5"]);
    assert!(
        result.is_err(),
        "positional HOST + --host must be rejected (clap conflicts_with)"
    );
}

#[test]
fn send_with_zero_chunk_ms_is_rejected() {
    let result = parse(&["send", "192.168.0.10", "--chunk-ms", "0"]);
    let err = result.expect_err("--chunk-ms 0 must error");
    assert_eq!(
        err.kind(),
        ErrorKind::ValueValidation,
        "expected ValueValidation, got {:?}",
        err.kind()
    );
}

#[test]
fn send_with_out_of_range_port_is_rejected() {
    let result = parse(&["send", "192.168.0.10", "--port", "70000"]);
    let err = result.expect_err("--port 70000 must error (above u16 max)");
    assert_eq!(
        err.kind(),
        ErrorKind::ValueValidation,
        "expected ValueValidation, got {:?}",
        err.kind()
    );
}

#[test]
fn recv_defaults_are_pinned() {
    let cli = parse(&["recv"]).expect("`recv` with no args is valid");
    let Command::Recv(args) = cli.command else {
        panic!("expected Recv variant");
    };
    assert_eq!(args.port, 5000);
    assert_eq!(args.prebuffer_ms, 200);
    assert!(!args.stats);
}

#[test]
fn recv_with_zero_prebuffer_ms_is_rejected() {
    let result = parse(&["recv", "--prebuffer-ms", "0"]);
    let err = result.expect_err("--prebuffer-ms 0 must error");
    assert_eq!(
        err.kind(),
        ErrorKind::ValueValidation,
        "expected ValueValidation, got {:?}",
        err.kind()
    );
}

#[test]
fn recv_with_custom_buffer_and_stats_flag() {
    let cli =
        parse(&["recv", "--prebuffer-ms", "500", "--stats"]).expect("recv with flags is valid");
    let Command::Recv(args) = cli.command else {
        panic!("expected Recv variant");
    };
    assert_eq!(args.prebuffer_ms, 500);
    assert!(args.stats);
}

#[test]
fn help_message_lists_all_three_subcommands() {
    // `--help` exits with DisplayHelp; the rendered help text mentions the
    // three primary subcommands.
    let result = parse(&["--help"]).expect_err("`--help` errors");
    let help_text = result.to_string();
    assert!(help_text.contains("send"), "help should list `send`");
    assert!(help_text.contains("recv"), "help should list `recv`");
    assert!(help_text.contains("devices"), "help should list `devices`");
}

#[test]
fn send_help_describes_required_host_argument() {
    let result = parse(&["send", "--help"]).expect_err("`send --help` errors");
    let help_text = result.to_string();
    assert!(
        help_text.contains("--host") || help_text.contains("<HOST>") || help_text.contains("host"),
        "send help should reference host argument, got: {help_text}"
    );
}

// ----- Slice 12 mDNS CLI surface (integration parse) -----

#[test]
fn send_mdns_parses_without_host() {
    let cli = parse(&["send", "--mdns"]).expect("send --mdns parses");
    let Command::Send(args) = cli.command else { panic!("send"); };
    assert!(args.mdns);
    assert!(args.host.is_none() && args.host_flag.is_none());
    let target = args.validate().expect("validate accepts mdns");
    assert!(matches!(target, teehee::cli::ResolvedTarget::Mdns { .. }));
}

#[test]
fn send_mdns_rejects_with_host_at_validate() {
    let cli = parse(&["send", "--mdns", "--host", "1.2.3.4"]).expect("clap allows (conflicts not on mdns)");
    let Command::Send(args) = cli.command else { panic!(); };
    let err = args.validate().expect_err("mdns+host rejected in validate");
    assert!(err.contains("--mdns"));
}

#[test]
fn recv_mdns_parses() {
    let cli = parse(&["recv", "--mdns", "--port", "6000"]).expect("recv --mdns ok");
    let Command::Recv(args) = cli.command else { panic!(); };
    assert!(args.mdns);
    assert_eq!(args.port, 6000);
}
