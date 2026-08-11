#![no_std]
#![no_main]
#![allow(clippy::println_empty_string)]

extern crate alloc;

#[macro_use]
extern crate user_lib;

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::mem;

use user_lib::{
    OpenFlags, SIG_IGN, TTY_CTL_SET_FLAGS, TTY_ISIG, close, dup, exec, fork, getpgrp, kill, open,
    pipe, read, setpgid, sigaction, tcsetpgrp, tty_ctl, waitpid, waitpid_nb, write,
};

const PROMPT: &str = ">> ";
const MAX_HISTORY: usize = 100;
const SIGINT_BITS: i32 = 1 << 2;
const ESC: u8 = 0x1b;
const CR: u8 = 0x0d;
const LF: u8 = 0x0a;
const BS: u8 = 0x08;
const DEL: u8 = 0x7f;

#[derive(Clone)]
enum JobStatus {
    Running,
    Done(i32),
}

struct Job {
    jid: usize,
    pgid: usize,
    pids: Vec<usize>,
    cmdline: String,
    status: JobStatus,
}

#[derive(Default)]
struct Stage {
    prog: String,
    args: Vec<String>,
    input: Option<String>,
    output: Option<String>,
}

struct Command {
    stages: Vec<Stage>,
    background: bool,
}

struct LineEditor {
    buf: Vec<u8>,
    cursor: usize,
    history: VecDeque<Vec<u8>>,
    hist_idx: Option<usize>,
}

impl LineEditor {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
            history: VecDeque::new(),
            hist_idx: None,
        }
    }

    fn refresh(&self) {
        print!("\r\x1b[K{}", PROMPT);
        write(1, &self.buf);
        if self.cursor < self.buf.len() {
            print!("\x1b[{}D", self.buf.len() - self.cursor);
        }
    }

    fn insert(&mut self, c: u8) {
        self.buf.insert(self.cursor, c);
        self.cursor += 1;
        self.refresh();
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.buf.remove(self.cursor - 1);
            self.cursor -= 1;
            self.refresh();
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.refresh();
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
            self.refresh();
        }
    }

    fn home(&mut self) {
        self.cursor = 0;
        self.refresh();
    }

    fn end(&mut self) {
        self.cursor = self.buf.len();
        self.refresh();
    }

    fn kill_line(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.refresh();
    }

    fn kill_to_end(&mut self) {
        self.buf.truncate(self.cursor);
        self.refresh();
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        self.hist_idx = Some(match self.hist_idx {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        });
        self.buf = self.history[self.hist_idx.unwrap()].clone();
        self.cursor = self.buf.len();
        self.refresh();
    }

    fn history_next(&mut self) {
        match self.hist_idx {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.hist_idx = Some(i + 1);
                self.buf = self.history[i + 1].clone();
                self.cursor = self.buf.len();
                self.refresh();
            }
            Some(_) => {
                self.hist_idx = None;
                self.buf.clear();
                self.cursor = 0;
                self.refresh();
            }
        }
    }

    fn add_history(&mut self, line: &[u8]) {
        if line.is_empty() {
            return;
        }
        if self
            .history
            .back()
            .map(|last| last.as_slice() == line)
            .unwrap_or(false)
        {
            return;
        }
        self.history.push_back(line.to_vec());
        if self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
        self.hist_idx = None;
    }

    fn readline(&mut self) -> Option<Vec<u8>> {
        self.refresh();
        loop {
            let mut c = [0u8; 1];
            let n = read(0, &mut c);
            if n <= 0 {
                return None;
            }
            match c[0] {
                LF | CR => {
                    write(1, b"\r\n");
                    let line = mem::take(&mut self.buf);
                    self.cursor = 0;
                    self.hist_idx = None;
                    return Some(line);
                }
                BS | DEL => self.backspace(),
                0x01 => self.home(),        // Ctrl-A
                0x05 => self.end(),         // Ctrl-E
                0x15 => self.kill_line(),   // Ctrl-U
                0x0b => self.kill_to_end(), // Ctrl-K
                0x04 => {
                    if self.buf.is_empty() {
                        write(1, b"\r\n");
                        return None;
                    }
                }
                ESC => {
                    let mut seq = [0u8; 2];
                    if read(0, &mut seq[0..1]) <= 0 || seq[0] != b'[' {
                        continue;
                    }
                    if read(0, &mut seq[1..2]) <= 0 {
                        continue;
                    }
                    match seq[1] {
                        b'A' => self.history_prev(),
                        b'B' => self.history_next(),
                        b'C' => self.move_right(),
                        b'D' => self.move_left(),
                        b'H' => self.home(),
                        b'F' => self.end(),
                        _ => {}
                    }
                }
                c if c >= 0x20 => self.insert(c),
                _ => {}
            }
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<String>, &'static str> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in input.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            ';' | '|' | '&' | '<' | '>' => {
                if !cur.is_empty() {
                    tokens.push(mem::take(&mut cur));
                }
                tokens.push(c.to_string());
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    tokens.push(mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if escaped || quote.is_some() {
        return Err("unbalanced quotes or trailing escape");
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    Ok(tokens)
}

fn finish_stage(stages: &mut Vec<Stage>, stage: &mut Stage) -> Result<(), &'static str> {
    if stage.prog.is_empty() {
        return Err("empty command in pipeline");
    }
    stages.push(mem::take(stage));
    Ok(())
}

fn parse_command(input: &str) -> Result<Vec<Command>, &'static str> {
    let tokens = tokenize(input)?;
    let mut commands = Vec::new();
    let mut stages: Vec<Stage> = Vec::new();
    let mut stage = Stage::default();
    let mut background = false;
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            ";" | "&" => {
                finish_stage(&mut stages, &mut stage)?;
                if tokens[i] == "&" {
                    background = true;
                }
                commands.push(Command {
                    stages: mem::take(&mut stages),
                    background,
                });
                background = false;
                stage = Stage::default();
            }
            "|" => {
                finish_stage(&mut stages, &mut stage)?;
            }
            "<" | ">" => {
                i += 1;
                if i >= tokens.len() {
                    return Err("missing file for redirection");
                }
                if tokens[i - 1] == "<" {
                    stage.input = Some(tokens[i].clone());
                } else {
                    stage.output = Some(tokens[i].clone());
                }
            }
            "&&" | "||" => return Err("&& and || are not supported"),
            _ => {
                if stage.prog.is_empty() {
                    stage.prog = tokens[i].clone();
                } else {
                    stage.args.push(tokens[i].clone());
                }
            }
        }
        i += 1;
    }
    if !stage.prog.is_empty() || !stages.is_empty() {
        finish_stage(&mut stages, &mut stage)?;
    }
    if !stages.is_empty() {
        commands.push(Command { stages, background });
    }
    Ok(commands)
}

fn nul(s: &str) -> String {
    let mut r = String::new();
    r.push_str(s);
    r.push('\0');
    r
}

fn join_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        return path.trim_start_matches('/').to_string();
    }
    if cwd.is_empty() {
        path.to_string()
    } else {
        let mut r = String::new();
        r.push_str(cwd);
        r.push('/');
        r.push_str(path);
        r
    }
}

fn resolve_prog(cwd: &str, prog: &str) -> String {
    if prog.contains('/') {
        prog.trim_start_matches('/').to_string()
    } else {
        join_path(cwd, prog)
    }
}

struct Shell {
    cwd: String,
    history: VecDeque<String>,
    jobs: Vec<Job>,
    next_jid: usize,
    shell_pgrp: usize,
}

impl Shell {
    fn new(shell_pgrp: usize) -> Self {
        Self {
            cwd: String::new(),
            history: VecDeque::new(),
            jobs: Vec::new(),
            next_jid: 1,
            shell_pgrp,
        }
    }

    fn add_history(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        if self
            .history
            .back()
            .map(|last| last == line)
            .unwrap_or(false)
        {
            return;
        }
        self.history.push_back(line.to_string());
        if self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
    }

    fn reap_jobs(&mut self) {
        let mut done: Vec<(usize, i32, String)> = Vec::new();
        for job in self.jobs.iter_mut() {
            let mut finished = Vec::new();
            let mut last_code = 0;
            for &pid in job.pids.iter() {
                let mut code = 0;
                let ret = waitpid_nb(pid, &mut code);
                if ret == pid as isize || ret == -1 {
                    finished.push(pid);
                    last_code = code;
                }
            }
            for pid in finished {
                job.pids.retain(|&p| p != pid);
            }
            if job.pids.is_empty() {
                done.push((job.jid, last_code, job.cmdline.clone()));
                job.status = JobStatus::Done(last_code);
            }
        }
        for (jid, code, cmdline) in done {
            println!("[{}]+ Done ({}) {}", jid, code, cmdline);
        }
        self.jobs
            .retain(|j| !matches!(j.status, JobStatus::Done(_)));
    }

    fn register_job(&mut self, pgid: usize, pids: Vec<usize>, cmdline: &str, background: bool) {
        let jid = self.next_jid;
        self.next_jid += 1;
        if background {
            println!("[{}] {}", jid, pgid);
        }
        self.jobs.push(Job {
            jid,
            pgid,
            pids,
            cmdline: cmdline.to_string(),
            status: JobStatus::Running,
        });
    }

    fn run_pipeline(&mut self, cmd: &Command) {
        let stages = &cmd.stages;
        let n = stages.len();
        let mut pipes: Vec<[usize; 2]> = Vec::new();
        for _ in 0..n.saturating_sub(1) {
            let mut fd = [0usize; 2];
            pipe(&mut fd);
            pipes.push(fd);
        }
        let mut children: Vec<usize> = Vec::new();
        let mut pgid = 0usize;
        for (i, stage) in stages.iter().enumerate() {
            let pid = fork();
            if pid == 0 {
                // child: join the pipeline's process group
                if i == 0 {
                    setpgid(0, 0);
                } else {
                    setpgid(0, pgid);
                }
                // redirections
                if let Some(path) = &stage.input {
                    let path = nul(path);
                    let fd = open(path.as_str(), OpenFlags::RDONLY);
                    if fd == -1 {
                        println!("shell: cannot open {}", path);
                        user_lib::exit(1);
                    }
                    let fd = fd as usize;
                    close(0);
                    assert_eq!(dup(fd), 0);
                    close(fd);
                }
                if let Some(path) = &stage.output {
                    let path = nul(path);
                    let fd = open(path.as_str(), OpenFlags::CREATE | OpenFlags::WRONLY);
                    if fd == -1 {
                        println!("shell: cannot open {}", path);
                        user_lib::exit(1);
                    }
                    let fd = fd as usize;
                    close(1);
                    assert_eq!(dup(fd), 1);
                    close(fd);
                }
                // pipeline fds
                if i > 0 {
                    close(0);
                    assert_eq!(dup(pipes[i - 1][0]), 0);
                }
                if i + 1 < n {
                    close(1);
                    assert_eq!(dup(pipes[i][1]), 1);
                }
                for pipe_fd in pipes.iter() {
                    close(pipe_fd[0]);
                    close(pipe_fd[1]);
                }
                // builtins can appear as pipeline stages (e.g. echo hi | wc)
                if self.is_pipeline_builtin(&stage.prog) {
                    self.run_pipeline_builtin(stage);
                    user_lib::exit(0);
                }
                // exec
                let path = nul(&resolve_prog(&self.cwd, &stage.prog));
                let mut args_nul: Vec<String> = Vec::new();
                args_nul.push(path.clone());
                for arg in stage.args.iter() {
                    args_nul.push(nul(arg));
                }
                let mut args_addr: Vec<*const u8> = args_nul.iter().map(|s| s.as_ptr()).collect();
                args_addr.push(core::ptr::null());
                if exec(path.as_str(), args_addr.as_slice()) == -1 {
                    user_lib::exit(127);
                }
                unreachable!();
            } else {
                if i == 0 {
                    pgid = pid as usize;
                }
                setpgid(pid as usize, pgid);
                children.push(pid as usize);
            }
        }
        for pipe_fd in pipes.iter() {
            close(pipe_fd[0]);
            close(pipe_fd[1]);
        }
        let cmdline = command_string(cmd);
        if cmd.background {
            self.register_job(pgid, children, &cmdline, true);
            return;
        }
        // foreground job: put it on the terminal and wait
        tcsetpgrp(0, pgid);
        let mut last_code = 0;
        for pid in children.iter() {
            waitpid(*pid, &mut last_code);
        }
        tcsetpgrp(0, self.shell_pgrp);
        if last_code != 0 {
            println!("[shell] process exited with code {}", last_code);
        }
    }

    fn is_pipeline_builtin(&self, prog: &str) -> bool {
        matches!(prog, "echo" | "pwd" | "history" | "help" | "clear")
    }

    fn run_pipeline_builtin(&self, stage: &Stage) {
        match stage.prog.as_str() {
            "echo" => self.builtin_echo(&stage.args, &stage.output),
            "pwd" => self.builtin_pwd(),
            "history" => self.builtin_history(),
            "help" => self.builtin_help(),
            "clear" => self.builtin_clear(),
            _ => {}
        }
    }

    fn builtin_echo(&self, args: &[String], output: &Option<String>) {
        let mut s = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(arg);
        }
        if let Some(path) = output {
            let path = nul(path);
            let fd = open(path.as_str(), OpenFlags::CREATE | OpenFlags::WRONLY);
            if fd == -1 {
                println!("shell: cannot open {}", path);
                return;
            }
            write(fd as usize, s.as_bytes());
            close(fd as usize);
        } else {
            println!("{}", s);
        }
    }

    fn builtin_cd(&mut self, args: &[String]) {
        if args.is_empty() {
            self.cwd.clear();
            return;
        }
        self.cwd = join_path(&self.cwd, &args[0]);
    }

    fn builtin_pwd(&self) {
        if self.cwd.is_empty() {
            println!("/");
        } else {
            println!("/{}", self.cwd);
        }
    }

    fn builtin_history(&self) {
        for (i, line) in self.history.iter().enumerate() {
            println!("{:>4}  {}", i + 1, line);
        }
    }

    fn builtin_jobs(&mut self) {
        self.reap_jobs();
        for job in self.jobs.iter() {
            let status = match job.status {
                JobStatus::Running => "Running".to_string(),
                JobStatus::Done(code) => alloc::format!("Done({})", code),
            };
            println!("[{}] {} pgid={} {}", job.jid, status, job.pgid, job.cmdline);
        }
    }

    fn builtin_fg(&mut self, args: &[String]) {
        let idx = match args.first() {
            Some(s) => s
                .parse::<usize>()
                .ok()
                .and_then(|jid| self.jobs.iter().position(|j| j.jid == jid)),
            None => self.jobs.len().checked_sub(1),
        };
        let idx = match idx {
            Some(idx) => idx,
            None => {
                println!("fg: job not found");
                return;
            }
        };
        let pgid = self.jobs[idx].pgid;
        let pids = self.jobs[idx].pids.clone();
        let cmdline = self.jobs[idx].cmdline.clone();
        tcsetpgrp(0, pgid);
        let mut last_code = 0;
        for pid in pids.iter() {
            waitpid(*pid, &mut last_code);
        }
        tcsetpgrp(0, self.shell_pgrp);
        println!("[{}]+ Done {}", self.jobs[idx].jid, cmdline);
        self.jobs.remove(idx);
    }

    fn builtin_bg(&mut self, args: &[String]) {
        let idx = match args.first() {
            Some(s) => s
                .parse::<usize>()
                .ok()
                .and_then(|jid| self.jobs.iter().position(|j| j.jid == jid)),
            None => self.jobs.len().checked_sub(1),
        };
        match idx {
            Some(idx) => match self.jobs[idx].status {
                JobStatus::Running => println!("bg: job {} already running", self.jobs[idx].jid),
                JobStatus::Done(_) => println!("bg: job {} already done", self.jobs[idx].jid),
            },
            None => println!("bg: job not found"),
        }
    }

    fn builtin_kill(&self, args: &[String]) {
        if args.is_empty() {
            println!("kill: usage: kill <pid | -pgid>");
            return;
        }
        let arg = &args[0];
        if let Some(pid) = arg.strip_prefix('-').and_then(|s| s.parse::<usize>().ok()) {
            let encoded = (-(pid as isize)) as usize;
            let ret = kill(encoded, SIGINT_BITS);
            if ret != 0 {
                println!("kill: no such process group {}", pid);
            }
        } else if let Ok(pid) = arg.parse::<usize>() {
            let ret = kill(pid, SIGINT_BITS);
            if ret != 0 {
                println!("kill: no such process {}", pid);
            }
        } else {
            println!("kill: invalid argument {}", arg);
        }
    }

    fn execute(&mut self, cmd: &Command) {
        if cmd.stages.len() == 1 {
            let stage = &cmd.stages[0];
            if stage.prog == "exit" {
                user_lib::exit(0);
            }
            let builtin = match stage.prog.as_str() {
                "echo" => Some(self.builtin_echo(&stage.args, &stage.output)),
                "cd" => Some(self.builtin_cd(&stage.args)),
                "pwd" => Some(self.builtin_pwd()),
                "history" => Some(self.builtin_history()),
                "jobs" => Some(self.builtin_jobs()),
                "fg" => Some(self.builtin_fg(&stage.args)),
                "bg" => Some(self.builtin_bg(&stage.args)),
                "kill" => Some(self.builtin_kill(&stage.args)),
                "help" => Some(self.builtin_help()),
                "clear" => Some(self.builtin_clear()),
                _ => None,
            };
            if builtin.is_some() {
                if cmd.background {
                    // builtins run synchronously in the shell
                }
                return;
            }
        }
        self.run_pipeline(cmd);
    }

    fn builtin_help(&self) {
        println!("builtins: cd pwd echo history jobs fg bg kill help clear exit");
        println!("syntax: cmd [args] [< file] [> file] [| cmd ...] [&] [; cmd]");
    }

    fn builtin_clear(&self) {
        print!("\x1b[2J\x1b[H");
    }
}

fn command_string(cmd: &Command) -> String {
    let mut s = String::new();
    for (i, stage) in cmd.stages.iter().enumerate() {
        if i > 0 {
            s.push_str(" | ");
        }
        s.push_str(&stage.prog);
        for arg in stage.args.iter() {
            s.push(' ');
            s.push_str(arg);
        }
        if let Some(input) = &stage.input {
            s.push_str(" < ");
            s.push_str(input);
        }
        if let Some(output) = &stage.output {
            s.push_str(" > ");
            s.push_str(output);
        }
    }
    if cmd.background {
        s.push_str(" &");
    }
    s
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("Rust user shell (tty-shell)");
    setpgid(0, 0);
    let shell_pgrp = getpgrp() as usize;
    sigaction(SIGINT_BITS, SIG_IGN);
    // raw mode, no kernel echo; keep Ctrl-C handling enabled
    tty_ctl(0, TTY_CTL_SET_FLAGS, TTY_ISIG);
    tcsetpgrp(0, shell_pgrp);

    let mut shell = Shell::new(shell_pgrp);
    let mut editor = LineEditor::new();
    loop {
        // child programs may change TTY flags (e.g. tty_test); always restore
        // the shell's raw editing mode before reading the next command
        tty_ctl(0, TTY_CTL_SET_FLAGS, TTY_ISIG);
        shell.reap_jobs();
        let line = match editor.readline() {
            Some(line) => line,
            None => break,
        };
        let line = String::from_utf8_lossy(&line).into_owned();
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        shell.add_history(&line);
        editor.add_history(line.as_bytes());
        match parse_command(&line) {
            Ok(commands) => {
                for cmd in commands {
                    shell.execute(&cmd);
                }
            }
            Err(e) => println!("shell: {}", e),
        }
    }
    println!("exit");
    0
}
