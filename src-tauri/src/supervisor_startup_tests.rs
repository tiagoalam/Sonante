// Synthetic executable only: no MPD, audio device, GUI or personal profile.
fn startup_fixture(mode: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let (root, socket, pid) = test_runtime_paths("startup-compatibility");
    let program = root.join("synthetic-engine.py");
    let log = root.join("attempts.jsonl");
    let script = format!(r##"#!/usr/bin/python3
import os,sys,json,socket,time
mode={mode}
log={log}
sock_path={socket}
pid_path={pid}
probe=sys.argv[1:]==['--version']
records=[]
if os.path.exists(log):
    with open(log) as f: records=[json.loads(line) for line in f]
record={{'kind':'probe' if probe else 'start','blas':os.environ.get('OPENBLAS_NUM_THREADS'),'omp':os.environ.get('OMP_NUM_THREADS')}}
with open(log,'a') as f: f.write(json.dumps(record)+'\n')
message='exception: Background threads were detected before MPD could initialize.'
if probe:
    if mode=='probe_timeout': time.sleep(30)
    if mode=='probe_flood':
        sys.stderr.write('x'*70000);sys.stderr.flush();sys.exit(1)
    if mode=='wrong_probe':
        print('codec: '+message,file=sys.stderr);sys.exit(1)
    if mode in ['guard','always_guard','cleanup_refused']:
        print(message,file=sys.stderr);sys.exit(1)
    print('Synthetic version');sys.exit(0)
if mode=='exit_success': sys.exit(0)
if mode=='unrelated':
    print('unrelated permission failure',file=sys.stderr);sys.exit(1)
starts=sum(row['kind']=='start' for row in records)+1
if mode in ['guard','always_guard','probe_timeout','probe_flood','wrong_probe','cleanup_refused']:
    if starts==1 or mode=='always_guard' or record['blas']!='1' or record['omp']!='1':
        print(message,file=sys.stderr);sys.exit(1)
server=socket.socket(socket.AF_UNIX)
server.bind(sock_path)
with open(pid_path,'w') as f: f.write(str(os.getpid()))
server.listen(4)
while True:
    connection,_=server.accept()
    with connection:
        connection.sendall(b'NOT MPD\n' if mode=='wrong_handshake' else b'OK MPD 0.24.15\n')
        command=connection.recv(1024)
        if command.startswith(b'kill'):
            connection.sendall(b'OK\n');break
"##,
        mode=serde_json::to_string(mode).unwrap(), log=serde_json::to_string(&log).unwrap(),
        socket=serde_json::to_string(&socket).unwrap(), pid=serde_json::to_string(&pid).unwrap());
    fs::write(&program, script).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("preserved-library"), b"unchanged synthetic catalogue").unwrap();
    (root, socket, pid, program)
}
fn startup_attempts(root: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(root.join("attempts.jsonl")).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect()
}
fn startup_attempt(root: &Path, socket: &Path, pid: &Path, program: PathBuf) -> (MpdSupervisor, Result<(), String>) {
    let mut supervisor = MpdSupervisor::new_with_runtime_paths(socket, pid);
    supervisor.prepare_runtime_files().unwrap();
    let engine = crate::mpd_engine::MpdEngine::startup_fixture(program, None);
    let result = supervisor.start_owned_engine(&engine, &root.join("unused-fixture.conf"), &AppConfig::default(), None);
    (supervisor, result)
}
#[test]
fn startup_thread_compatibility_normal_launch_keeps_environment_and_skips_probe() {
    let (root,socket,pid,program)=startup_fixture("good");
    let (mut supervisor,result)=startup_attempt(&root,&socket,&pid,program);result.unwrap();
    let attempts=startup_attempts(&root);assert_eq!(attempts.len(),1);assert_eq!(attempts[0]["kind"],"start");
    assert_eq!(attempts[0]["blas"],serde_json::to_value(std::env::var("OPENBLAS_NUM_THREADS").ok()).unwrap());
    assert_eq!(attempts[0]["omp"],serde_json::to_value(std::env::var("OMP_NUM_THREADS").ok()).unwrap());
    supervisor.stop().unwrap();
    assert_eq!(fs::read(root.join("preserved-library")).unwrap(),b"unchanged synthetic catalogue");
}
#[test]
fn startup_thread_compatibility_recovers_once_without_parent_changes() {
    const CASE:&str="SONANTE_STARTUP_COMPATIBILITY_ISOLATED";
    if std::env::var(CASE).as_deref()!=Ok("1") {
        let result=Command::new(std::env::current_exe().unwrap())
            .args(["--exact","supervisor::tests::startup_thread_compatibility_recovers_once_without_parent_changes","--test-threads=1"])
            .env(CASE,"1").env("OPENBLAS_NUM_THREADS","9").env("OMP_NUM_THREADS","7").output().unwrap();
        assert!(result.status.success(),"{}{}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"));return;
    }
    let (root,socket,pid,program)=startup_fixture("guard");
    let (mut supervisor,result)=startup_attempt(&root,&socket,&pid,program);result.unwrap();
    let attempts=startup_attempts(&root);assert_eq!(attempts.len(),3);
    assert_eq!(attempts.iter().map(|x|x["kind"].as_str().unwrap()).collect::<Vec<_>>(),vec!["start","probe","start"]);
    for attempt in &attempts[..2] {assert_eq!(attempt["blas"],"9");assert_eq!(attempt["omp"],"7");}
    assert_eq!(attempts[2]["blas"],"1");assert_eq!(attempts[2]["omp"],"1");
    assert_eq!(std::env::var("OPENBLAS_NUM_THREADS").unwrap(),"9");assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(),"7");
    supervisor.stop().unwrap();assert!(supervisor.process.is_none());
    assert_eq!(fs::read(root.join("preserved-library")).unwrap(),b"unchanged synthetic catalogue");
}
#[test]
fn startup_thread_compatibility_never_retries_twice_or_other_failures() {
    for (mode,count) in [("always_guard",3),("unrelated",2),("exit_success",1),("wrong_probe",2),("wrong_handshake",1)] {
        let (root,socket,pid,program)=startup_fixture(mode);
        let (mut supervisor,result)=startup_attempt(&root,&socket,&pid,program);assert!(result.is_err(),"{mode}");
        assert!(supervisor.process.is_none(),"{mode}");
        assert_eq!(startup_attempts(&root).len(),count,"{mode}");supervisor.stop().unwrap();
    }
}
#[test]
fn startup_thread_compatibility_probe_is_bounded_and_reaped() {
    for (mode,code) in [("probe_timeout","MPD_STARTUP_PROBE_TIMEOUT"),("probe_flood","MPD_STARTUP_PROBE_OUTPUT_LIMIT")] {
        let (root,socket,pid,program)=startup_fixture(mode);let start=Instant::now();
        let (mut supervisor,result)=startup_attempt(&root,&socket,&pid,program);
        assert!(result.unwrap_err().contains(code));assert!(supervisor.process.is_none());
        assert_eq!(startup_attempts(&root).len(),2);assert!(start.elapsed()<Duration::from_secs(5));
        supervisor.stop().unwrap();
    }
}
#[test]
fn startup_thread_compatibility_cleanup_failure_prevents_probe() {
    let (root,socket,pid,program)=startup_fixture("cleanup_refused");
    let mut supervisor=MpdSupervisor::new_with_runtime_paths(&socket,&pid);
    let fifo=supervisor.analyzer_fifo_path_for_instance();fs::create_dir(&fifo).unwrap();
    supervisor.owns_analyzer_fifo=true;
    let engine=crate::mpd_engine::MpdEngine::startup_fixture(program, None);
    assert!(supervisor.start_owned_engine(&engine,&root.join("fixture.conf"),&AppConfig::default(),None).is_err());
    assert_eq!(startup_attempts(&root).len(),1);assert!(fifo.is_dir());assert!(supervisor.process.is_none());
    fs::remove_dir(fifo).unwrap();supervisor.stop().unwrap();
}
#[test]
fn startup_thread_compatibility_recognizes_only_the_specific_guard() {
    let guard=b"exception: Background threads were detected before MPD could initialize. More explanation.\n";
    assert!(MpdSupervisor::is_early_thread_failure(guard));
    for other in [b"exception: permission denied".as_slice(),b"codec: Background threads were detected before MPD could initialize.",b"Background threads were detected before MPD could initialize.partial",b"\xffinvalid"] {
        assert!(!MpdSupervisor::is_early_thread_failure(other));
    }
}

#[test]
fn startup_thread_compatibility_identity_mismatch_never_probes() {
    let (root,socket,pid,program)=startup_fixture("good");
    let mut supervisor=MpdSupervisor::new_with_runtime_paths(&socket,&pid);
    supervisor.prepare_runtime_files().unwrap();
    let engine=crate::mpd_engine::MpdEngine::startup_fixture(program,Some("0".repeat(64)));
    let result=supervisor.start_owned_engine(&engine,&root.join("fixture.conf"),&AppConfig::default(),None);
    assert!(result.unwrap_err().contains("divergiu"));
    assert!(supervisor.process.is_none());
    // The hash guard may terminate Python before its first instruction.
    if root.join("attempts.jsonl").exists() { assert_eq!(startup_attempts(&root).len(),1); }
    supervisor.stop().unwrap();
}

// Like the existing first-run reopening fixtures, consecutive ledger
// acquisitions need their own process: unrelated parallel forks can retain an
// inherited flock until exec even after the test thread closed its descriptor.
fn isolated_startup_fixture_case(name: &str) -> bool {
    const CASE:&str="SONANTE_SUPERVISOR_ISOLATED_CASE";
    if std::env::var(CASE).as_deref()==Ok(name) { return false; }
    let result=Command::new(std::env::current_exe().unwrap())
        .args(["--exact",&format!("supervisor::tests::{name}"),"--test-threads=1"])
        .env(CASE,name).output().unwrap();
    assert!(result.status.success(),"{}{}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"));
    true
}
