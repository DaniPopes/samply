use crate::{
    linux_shared::{Converter, MmapRangeOrVec},
    shared::{process_sample_data::SimpleMarker, types::FastHashMap},
};
use framehop::{Module, Unwinder};
use fxprof_processed_profile::MarkerTiming;
use interprocess::local_socket::{
    tokio::{prelude::*, Listener, Stream},
    GenericFilePath, ListenerOptions, ToFsName,
};
use log::debug;
use serde::Deserialize;
use std::thread::JoinHandle;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;

type Pid = u32;
type Tid = u32;

pub struct IpcServerHandle {
    handle: JoinHandle<Processes>,
    shutdown_tx: oneshot::Sender<()>,
}

impl IpcServerHandle {
    pub fn spawn() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join("samply.sock");

        let name = path.as_path().to_fs_name::<GenericFilePath>()?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let listener =
            rt.block_on(async move { ListenerOptions::new().name(name).create_tokio() })?;

        debug!("listening on {}", path.display());

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = std::thread::Builder::new()
            .name("IPC Server".to_string())
            .spawn(move || rt.block_on(IpcServer::new().run(listener, shutdown_rx)))?;

        Ok(Self {
            handle,
            shutdown_tx,
        })
    }

    pub fn shutdown_and_aggregate<U>(self, converter: &mut Converter<U>)
    where
        U: Unwinder<Module = Module<MmapRangeOrVec>> + Default,
    {
        let _ = self.shutdown_tx.send(());
        let processes = self.handle.join().unwrap();
        for (pid, process) in processes {
            for (tid, thread) in process.threads {
                for span in thread.spans {
                    let timing = MarkerTiming::Interval(
                        converter.timestamp_converter().convert_time(span.start),
                        converter.timestamp_converter().convert_time(span.end),
                    );
                    let label = converter.handle_for_string(&span.label);
                    converter.profile.processes()[0].pid()
                    converter.add_marker(pid as _, tid as _, timing, SimpleMarker(label));
                }
            }
        }
    }
}

#[derive(Clone)]
struct IpcServer {
    _private: (),
}

impl IpcServer {
    fn new() -> Self {
        Self { _private: () }
    }

    async fn run(self, listener: Listener, mut shutdown_rx: oneshot::Receiver<()>) -> Processes {
        let mut handles = Vec::new();
        loop {
            tokio::select! {
                biased;

                _ = &mut shutdown_rx => break,

                result = listener.accept() => match result {
                    Ok(stream) => handles.push(tokio::spawn(self.make_shard().handle_connection(stream))),
                    Err(e) => debug!("IPC accept error: {e}"),
                },
            }
        }

        let mut map = Processes::default();
        for handle in handles {
            if !handle.is_finished() {
                // TODO: all profiler processes should have finished,
                // but this can be anything that connected to us
                handle.abort();
                continue;
            }
            let shard = handle.await.unwrap();
            let Some(pid) = shard.pid else { continue };
            map.insert(pid, shard.process);
        }
        map
    }

    fn make_shard(&self) -> IpcServerShard {
        IpcServerShard::new()
    }
}

struct IpcServerShard {
    pid: Option<Pid>,
    process: Process,
}

type Processes = FastHashMap<Pid, Process>;

#[derive(Default, Debug)]
struct Process {
    threads: FastHashMap<Tid, Thread>,
}

#[derive(Default, Debug)]
struct Thread {
    spans: Vec<Span>,
}

#[derive(Debug)]
struct Span {
    start: u64,
    end: u64,
    label: Box<str>,
}

impl IpcServerShard {
    fn new() -> Self {
        Self {
            pid: None,
            process: Process::default(),
        }
    }

    async fn handle_connection(mut self, stream: Stream) -> Self {
        let reader = BufReader::new(stream);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<IpcMessage>(line) {
                Ok(msg) => _ = self.handle_msg(msg),
                Err(e) => debug!("Failed to parse IPC event: {e}"),
            }
        }
        self
    }

    fn handle_msg(&mut self, msg: IpcMessage) {
        debug!("received: {msg:?}");
        match msg {
            IpcMessage::Init { pid } => {
                if self.pid.is_some() {
                    // already initialized
                    return;
                }
                self.pid = Some(pid);
            }

            IpcMessage::Span {
                tid,
                start,
                end,
                label,
            } => self
                .process
                .threads
                .entry(tid)
                .or_default()
                .spans
                .push(Span { start, end, label }),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum IpcMessage {
    Init {
        pid: u32,
    },
    Span {
        tid: u32,
        start: u64,
        end: u64,
        label: Box<str>,
    },
}
