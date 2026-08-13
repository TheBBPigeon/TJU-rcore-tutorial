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
    OpenFlags, SIG_IGN, TTY_CTL_SET_FLAGS, TTY_ISIG, chdir, close, dup, exec, fork, getcwd,
    getpgrp, kill, list_apps, open, pipe, read, setpgid, sigaction, sleep, tcsetpgrp, tty_ctl,
    waitpid_nb, waitpid_nb_opts, write,
};

fn make_prompt() -> String {
    let mut buf = [0u8; 256];
    let len = getcwd(&mut buf);
    let cwd = if len > 0 {
        core::str::from_utf8(&buf[..len as usize]).unwrap_or("/")
    } else {
        "/"
    };
    let mut prompt = String::from(cwd);
    prompt.push_str(">> ");
    prompt
}
const MAX_HISTORY: usize = 100;
const SIGINT_BITS: i32 = 1 << 2;
const SIGTSTP_BITS: i32 = 1 << 20;
const SIGCONT_BITS: i32 = 1 << 21;
const WUNTRACED: usize = 1;
const ESC: u8 = 0x1b;
const CR: u8 = 0x0d;
const LF: u8 = 0x0a;
const BS: u8 = 0x08;
const DEL: u8 = 0x7f;

#[derive(Clone)]
enum JobStatus {
    Running,
    Stopped,
    Done(i32),
}

enum WaitResult {
    Reaped(i32),
    Stopped,
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
    stderr: Option<String>,
    append: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Connector {
    None,
    And,
    Or,
}

struct Command {
    stages: Vec<Stage>,
    background: bool,
    connector: Connector,
}

struct LineEditor {
    buf: Vec<u8>,
    cursor: usize,
    history: VecDeque<Vec<u8>>,
    hist_idx: Option<usize>,
    app_names: Vec<String>,
}

impl LineEditor {
    fn new(app_names: Vec<String>) -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
            history: VecDeque::new(),
            hist_idx: None,
            app_names,
        }
    }

    fn refresh(&self) {
        print!("\r\x1b[K{}", make_prompt());
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

    fn delete_char(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
            self.refresh();
        }
    }

    fn delete_word(&mut self) {
        while self.cursor > 0 && self.buf[self.cursor - 1].is_ascii_whitespace() {
            self.buf.remove(self.cursor - 1);
            self.cursor -= 1;
        }
        while self.cursor > 0 && !self.buf[self.cursor - 1].is_ascii_whitespace() {
            self.buf.remove(self.cursor - 1);
            self.cursor -= 1;
        }
        self.refresh();
    }

    fn complete(&mut self) {
        let start = self.buf[..self.cursor]
            .iter()
            .rposition(|&b| b == b' ' || b == b'|' || b == b'<')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = String::from_utf8_lossy(&self.buf[start..self.cursor]).into_owned();
        let mut matches: Vec<String> = self
            .app_names
            .iter()
            .filter(|n| n.starts_with(&prefix))
            .cloned()
            .collect();
        for b in [
            "cd", "pwd", "echo", "exit", "history", "jobs", "fg", "bg", "kill", "help", "clear",
        ] {
            if b.starts_with(&prefix) && !matches.iter().any(|m| m == b) {
                matches.push(b.to_string());
            }
        }
        if matches.is_empty() {
            return;
        }
        if matches.len() == 1 {
            let name = matches[0].clone();
            self.buf.splice(start..self.cursor, name.bytes());
            self.cursor = start + name.len();
            self.buf.insert(self.cursor, b' ');
            self.cursor += 1;
            self.refresh();
        } else {
            println!("");
            for m in matches.iter() {
                print!("{}  ", m);
            }
            println!("");
            self.refresh();
        }
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
                0x17 => self.delete_word(), // Ctrl-W
                b'\t' => self.complete(),
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
                        b'3' => {
                            let mut end = [0u8; 1];
                            if read(0, &mut end) <= 0 || end[0] != b'~' {
                                continue;
                            }
                            self.delete_char();
                        }
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
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
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
        let two_char_op = match c {
            '&' if chars.peek() == Some(&'&') => Some("&&"),
            '|' if chars.peek() == Some(&'|') => Some("||"),
            '>' if chars.peek() == Some(&'>') => Some(">>"),
            '2' if chars.peek() == Some(&'>') => Some("2>"),
            _ => None,
        };
        if let Some(op) = two_char_op {
            if !cur.is_empty() {
                tokens.push(mem::take(&mut cur));
            }
            tokens.push(op.to_string());
            chars.next();
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
    let mut pending = Connector::None;
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            ";" | "&" | "&&" | "||" => {
                finish_stage(&mut stages, &mut stage)?;
                if tokens[i] == "&" {
                    background = true;
                }
                commands.push(Command {
                    stages: mem::take(&mut stages),
                    background,
                    connector: pending,
                });
                background = false;
                stage = Stage::default();
                pending = match tokens[i].as_str() {
                    "&&" => Connector::And,
                    "||" => Connector::Or,
                    _ => Connector::None,
                };
            }
            "|" => {
                finish_stage(&mut stages, &mut stage)?;
            }
            "<" | ">" | ">>" | "2>" => {
                i += 1;
                if i >= tokens.len() {
                    return Err("missing file for redirection");
                }
                match tokens[i - 1].as_str() {
                    "<" => stage.input = Some(tokens[i].clone()),
                    ">" => {
                        stage.output = Some(tokens[i].clone());
                        stage.append = false;
                    }
                    ">>" => {
                        stage.output = Some(tokens[i].clone());
                        stage.append = true;
                    }
                    "2>" => stage.stderr = Some(tokens[i].clone()),
                    _ => unreachable!(),
                }
            }
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
        commands.push(Command {
            stages,
            background,
            connector: pending,
        });
    }
    Ok(commands)
}

fn nul(s: &str) -> String {
    let mut r = String::new();
    r.push_str(s);
    r.push('\0');
    r
}

/// Wait for a foreground child without busy-spinning the CPU. Reports whether
/// the child was reaped or stopped by SIGTSTP (WUNTRACED).
fn wait_foreground(pid: usize, exit_code: &mut i32) -> WaitResult {
    loop {
        match waitpid_nb_opts(pid, exit_code, WUNTRACED) {
            -2 => sleep(2),
            -3 => return WaitResult::Stopped,
            _ => return WaitResult::Reaped(*exit_code),
        }
    }
}

struct Shell {
    history: VecDeque<String>,
    jobs: Vec<Job>,
    next_jid: usize,
    shell_pgrp: usize,
    history_cleared: bool,
}

impl Shell {
    fn new(shell_pgrp: usize) -> Self {
        Self {
            history: VecDeque::new(),
            jobs: Vec::new(),
            next_jid: 1,
            shell_pgrp,
            history_cleared: false,
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

    fn register_stopped_job(&mut self, pgid: usize, pids: Vec<usize>, cmdline: &str) {
        let jid = self.next_jid;
        self.next_jid += 1;
        println!("[{}]+ Stopped {}", jid, cmdline);
        self.jobs.push(Job {
            jid,
            pgid,
            pids,
            cmdline: cmdline.to_string(),
            status: JobStatus::Stopped,
        });
    }

    fn run_pipeline(&mut self, cmd: &Command) -> i32 {
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
                    let mut flags = OpenFlags::CREATE | OpenFlags::WRONLY;
                    if stage.append {
                        flags |= OpenFlags::APPEND;
                    }
                    let fd = open(path.as_str(), flags);
                    if fd == -1 {
                        println!("shell: cannot open {}", path);
                        user_lib::exit(1);
                    }
                    let fd = fd as usize;
                    close(1);
                    assert_eq!(dup(fd), 1);
                    close(fd);
                }
                if let Some(path) = &stage.stderr {
                    let path = nul(path);
                    let fd = open(path.as_str(), OpenFlags::CREATE | OpenFlags::WRONLY);
                    if fd == -1 {
                        println!("shell: cannot open {}", path);
                        user_lib::exit(1);
                    }
                    let fd = fd as usize;
                    close(2);
                    assert_eq!(dup(fd), 2);
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
                // exec: the kernel's sys_exec searches PATH for bare names and
                // resolves absolute/relative paths from the process CWD.
                let path = nul(&stage.prog);
                let mut args_nul: Vec<String> = Vec::new();
                args_nul.push(path.clone());
                for arg in stage.args.iter() {
                    args_nul.push(nul(arg));
                }
                let mut args_addr: Vec<*const u8> = args_nul.iter().map(|s| s.as_ptr()).collect();
                args_addr.push(core::ptr::null());
                if exec(path.as_str(), args_addr.as_slice()) == -1 {
                    let msg = alloc::format!("shell: command not found: {}\n", stage.prog);
                    write(2, msg.as_bytes());
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
            return 0;
        }
        // foreground job: put it on the terminal and wait
        tcsetpgrp(0, pgid);
        let mut last_code = 0;
        let mut stopped = false;
        for pid in children.iter() {
            match wait_foreground(*pid, &mut last_code) {
                WaitResult::Reaped(code) => last_code = code,
                WaitResult::Stopped => stopped = true,
            }
        }
        tcsetpgrp(0, self.shell_pgrp);
        if stopped {
            self.register_stopped_job(pgid, children, &cmdline);
            return 1;
        }
        if last_code != 0 {
            println!("[shell] process exited with code {}", last_code);
        }
        last_code
    }

    fn is_pipeline_builtin(&self, prog: &str) -> bool {
        matches!(prog, "echo" | "pwd" | "history" | "help" | "clear")
    }

    fn run_pipeline_builtin(&mut self, stage: &Stage) {
        match stage.prog.as_str() {
            "echo" => self.builtin_echo(&stage.args, &stage.output, stage.append),
            "pwd" => self.builtin_pwd(),
            "history" => self.builtin_history(&[]),
            "help" => self.builtin_help(),
            "clear" => self.builtin_clear(),
            _ => {}
        }
    }

    fn builtin_echo(&self, args: &[String], output: &Option<String>, append: bool) {
        let mut s = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(arg);
        }
        if let Some(path) = output {
            let path = nul(path);
            let mut flags = OpenFlags::CREATE | OpenFlags::WRONLY;
            if append {
                flags |= OpenFlags::APPEND;
            }
            let fd = open(path.as_str(), flags);
            if fd == -1 {
                println!("shell: cannot open {}", path);
                return;
            }
            write(fd as usize, s.as_bytes());
            write(fd as usize, b"\n");
            close(fd as usize);
        } else {
            println!("{}", s);
        }
    }

    fn builtin_cd(&self, args: &[String]) {
        let path = if args.is_empty() { "/" } else { args[0].as_str() };
        if chdir(path) != 0 {
            println!("cd: {}: No such file or directory", path);
        }
    }

    fn builtin_pwd(&self) {
        let mut buf = [0u8; 256];
        let len = getcwd(&mut buf);
        if len > 0 {
            let cwd = core::str::from_utf8(&buf[..len as usize]).unwrap_or("/");
            println!("{}", cwd);
        } else {
            println!("/");
        }
    }

    fn builtin_history(&mut self, args: &[String]) {
        if args.first().map(|s| s.as_str()) == Some("-c") {
            self.history.clear();
            self.history_cleared = true;
            return;
        }
        for (i, line) in self.history.iter().enumerate() {
            println!("{:>4}  {}", i + 1, line);
        }
    }

    fn builtin_jobs(&mut self) {
        self.reap_jobs();
        for job in self.jobs.iter() {
            let status = match &job.status {
                JobStatus::Running => "Running".to_string(),
                JobStatus::Stopped => "Stopped".to_string(),
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
        let jid = self.jobs[idx].jid;
        if matches!(self.jobs[idx].status, JobStatus::Stopped) {
            let encoded = (-(pgid as isize)) as usize;
            if kill(encoded, SIGCONT_BITS) != 0 {
                println!("fg: failed to continue job {}", jid);
                return;
            }
            self.jobs[idx].status = JobStatus::Running;
        }
        tcsetpgrp(0, pgid);
        let mut last_code = 0;
        let mut stopped_again = false;
        for pid in pids.iter() {
            match wait_foreground(*pid, &mut last_code) {
                WaitResult::Reaped(code) => last_code = code,
                WaitResult::Stopped => stopped_again = true,
            }
        }
        tcsetpgrp(0, self.shell_pgrp);
        if stopped_again {
            println!("[{}]+ Stopped {}", jid, cmdline);
            self.jobs[idx].status = JobStatus::Stopped;
        } else {
            println!("[{}]+ Done {}", jid, cmdline);
            self.jobs.remove(idx);
        }
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
            Some(idx) => {
                let jid = self.jobs[idx].jid;
                match &self.jobs[idx].status {
                    JobStatus::Running => println!("bg: job {} already running", jid),
                    JobStatus::Stopped => {
                        let pgid = self.jobs[idx].pgid;
                        let encoded = (-(pgid as isize)) as usize;
                        if kill(encoded, SIGCONT_BITS) != 0 {
                            println!("bg: failed to continue job {}", jid);
                            return;
                        }
                        self.jobs[idx].status = JobStatus::Running;
                        println!("[{}] {}", jid, self.jobs[idx].cmdline);
                    }
                    JobStatus::Done(_) => println!("bg: job {} already done", jid),
                }
            }
            None => println!("bg: job not found"),
        }
    }

    fn builtin_kill(&self, args: &[String]) {
        if args.is_empty() {
            println!("kill: usage: kill [-SIG] <pid | -pgid>");
            return;
        }
        let mut signal = SIGINT_BITS;
        let mut target = 0usize;
        if let Some(name) = args[0].strip_prefix('-') {
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
                target = 0;
            } else {
                signal = match name.to_ascii_uppercase().as_str() {
                    "INT" => SIGINT_BITS,
                    "TSTP" => SIGTSTP_BITS,
                    "CONT" => SIGCONT_BITS,
                    "ABRT" => 1 << 6,
                    "ILL" => 1 << 4,
                    "FPE" => 1 << 8,
                    "SEGV" => 1 << 11,
                    _ => {
                        println!("kill: unknown signal {}", name);
                        return;
                    }
                };
                target = 1;
            }
        }
        if target >= args.len() {
            println!("kill: usage: kill [-SIG] <pid | -pgid>");
            return;
        }
        let arg = &args[target];
        if let Some(pid) = arg.strip_prefix('-').and_then(|s| s.parse::<usize>().ok()) {
            let encoded = (-(pid as isize)) as usize;
            let ret = kill(encoded, signal);
            if ret != 0 {
                println!("kill: no such process group {}", pid);
            }
        } else if let Ok(pid) = arg.parse::<usize>() {
            let ret = kill(pid, signal);
            if ret != 0 {
                println!("kill: no such process {}", pid);
            }
        } else {
            println!("kill: invalid argument {}", arg);
        }
    }

    fn execute(&mut self, cmd: &Command) -> i32 {
        if cmd.stages.len() == 1 {
            let stage = &cmd.stages[0];
            if stage.prog == "exit" {
                user_lib::exit(0);
            }
            let handled: Option<i32> = match stage.prog.as_str() {
                "echo" => {
                    self.builtin_echo(&stage.args, &stage.output, stage.append);
                    Some(0)
                }
                "cd" => {
                    self.builtin_cd(&stage.args);
                    Some(0)
                }
                "pwd" => {
                    self.builtin_pwd();
                    Some(0)
                }
                "history" => {
                    self.builtin_history(&stage.args);
                    Some(0)
                }
                "jobs" => {
                    self.builtin_jobs();
                    Some(0)
                }
                "fg" => {
                    self.builtin_fg(&stage.args);
                    Some(0)
                }
                "bg" => {
                    self.builtin_bg(&stage.args);
                    Some(0)
                }
                "kill" => {
                    self.builtin_kill(&stage.args);
                    Some(0)
                }
                "help" => {
                    self.builtin_help();
                    Some(0)
                }
                "clear" => {
                    self.builtin_clear();
                    Some(0)
                }
                _ => None,
            };
            if let Some(code) = handled {
                return code;
            }
        }
        self.run_pipeline(cmd)
    }

    fn builtin_help(&self) {
        println!("builtins: cd pwd echo history jobs fg bg kill help clear exit");
        println!(
            "syntax: cmd [args] [< file] [> file] [>> file] [2> file] [| cmd] [&] [;] [&&] [||]"
        );
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
    sigaction(SIGTSTP_BITS, SIG_IGN);
    // raw mode, no kernel echo; keep Ctrl-C handling enabled
    tty_ctl(0, TTY_CTL_SET_FLAGS, TTY_ISIG);
    tcsetpgrp(0, shell_pgrp);

    let mut shell = Shell::new(shell_pgrp);
    let mut editor = LineEditor::new(list_apps());
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
                let mut prev_status = 0i32;
                for cmd in commands {
                    match cmd.connector {
                        Connector::None => prev_status = shell.execute(&cmd),
                        Connector::And if prev_status == 0 => prev_status = shell.execute(&cmd),
                        Connector::And => {}
                        Connector::Or if prev_status != 0 => prev_status = shell.execute(&cmd),
                        Connector::Or => {}
                    }
                }
            }
            Err(e) => println!("shell: {}", e),
        }
        if shell.history_cleared {
            editor.history.clear();
            shell.history_cleared = false;
        }
    }
    println!("exit");
    0
}
