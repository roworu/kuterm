//! event bus: what happens in panes, sent to every subscriber as it happens. the remote
//! socket and mcp print the events as json, one per line

use std::{
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde::{Deserialize, Serialize};

/// something that happened in pane `pane`, the id `kuterm msg --pane` takes
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BusEvent {
    CommandStarted {
        pane: u64,
        block: u64,
        command: String,
    },
    CommandFinished {
        pane: u64,
        block: Option<u64>,
        exit: Option<i32>,
        seconds: f64,
    },
    CwdChanged {
        pane: u64,
        cwd: PathBuf,
    },
    /// the program in front of the shell changed
    ProcessChanged {
        pane: u64,
        name: String,
    },
    /// where the pane runs changed, like an ssh host or a container
    ContextChanged {
        pane: u64,
        context: String,
    },
    Bell {
        pane: u64,
    },
    Notify {
        pane: u64,
        title: String,
        body: String,
    },
    /// the program started or stopped reporting progress
    Progress {
        pane: u64,
        busy: bool,
    },
    TitleChanged {
        pane: u64,
        title: String,
    },
    /// output matched a watched pattern
    OutputMatched {
        pane: u64,
        pattern: String,
        text: String,
    },
    PaneOpened {
        pane: u64,
    },
    /// tty settings of the pane changed, like echo going off for a password prompt
    PtyModesChanged {
        pane: u64,
        echo: bool,
        icanon: bool,
        ixon: bool,
    },
    PaneClosed {
        pane: u64,
    },
    /// the pane got the focus of its window
    FocusChanged {
        pane: u64,
    },
}

impl BusEvent {
    /// the snake case name of the event, like "command_finished"
    pub fn name(&self) -> String {
        serde_json_lenient::to_value(self)
            .ok()
            .and_then(|value| value.get("event")?.as_str().map(str::to_string))
            .unwrap_or_default()
    }
}

/// the one bus of the app
pub struct EventBus {
    subscribers: Mutex<Vec<UnboundedSender<BusEvent>>>,
}

static BUS: LazyLock<EventBus> = LazyLock::new(|| EventBus {
    subscribers: Mutex::new(Vec::new()),
});

impl EventBus {
    /// a channel getting every event from now on, from any thread. a slow reader only
    /// queues up its own events, the ui never waits for it
    pub fn subscribe() -> UnboundedReceiver<BusEvent> {
        let (tx, rx) = unbounded();
        BUS.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// send `event` to every subscriber, forgetting the ones that are gone
    pub fn publish(event: BusEvent) {
        BUS.subscribers
            .lock()
            .unwrap()
            .retain(|subscriber| subscriber.unbounded_send(event.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::{BusEvent, EventBus};

    #[test]
    fn every_subscriber_gets_every_event_as_json() {
        let events = [
            BusEvent::CommandStarted {
                pane: 1,
                block: 2,
                command: "ls".into(),
            },
            BusEvent::CommandFinished {
                pane: 1,
                block: Some(2),
                exit: Some(0),
                seconds: 0.5,
            },
            BusEvent::CwdChanged {
                pane: 1,
                cwd: "/tmp".into(),
            },
            BusEvent::ProcessChanged {
                pane: 1,
                name: "vim".into(),
            },
            BusEvent::ContextChanged {
                pane: 1,
                context: "ssh host".into(),
            },
            BusEvent::Bell { pane: 1 },
            BusEvent::Notify {
                pane: 1,
                title: "t".into(),
                body: "b".into(),
            },
            BusEvent::Progress {
                pane: 1,
                busy: true,
            },
            BusEvent::TitleChanged {
                pane: 1,
                title: "x".into(),
            },
            BusEvent::OutputMatched {
                pane: 1,
                pattern: "err".into(),
                text: "error".into(),
            },
            BusEvent::PaneOpened { pane: 1 },
            BusEvent::PtyModesChanged {
                pane: 1,
                echo: false,
                icanon: true,
                ixon: true,
            },
            BusEvent::PaneClosed { pane: 1 },
            BusEvent::FocusChanged { pane: 1 },
        ];
        // other tests may publish too, so only events of this made up pane count
        let mut a = EventBus::subscribe();
        let mut b = EventBus::subscribe();
        for event in &events {
            let mut event = event.clone();
            set_pane(&mut event, 777_001);
            EventBus::publish(event);
        }
        for rx in [&mut a, &mut b] {
            let got: Vec<BusEvent> = std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|event| pane_of(event) == 777_001)
                .collect();
            assert_eq!(got.len(), events.len());
        }
        let json = serde_json_lenient::to_string(&events[1]).unwrap();
        assert_eq!(
            json,
            r#"{"event":"command_finished","pane":1,"block":2,"exit":0,"seconds":0.5}"#
        );
        assert_eq!(events[5].name(), "bell");
        // a dropped receiver is forgotten
        drop(b);
        EventBus::publish(BusEvent::Bell { pane: 777_001 });
    }

    fn pane_of(event: &BusEvent) -> u64 {
        serde_json_lenient::to_value(event).unwrap()["pane"]
            .as_u64()
            .unwrap()
    }

    fn set_pane(event: &mut BusEvent, id: u64) {
        let mut value = serde_json_lenient::to_value(&*event).unwrap();
        value["pane"] = id.into();
        *event = serde_json_lenient::from_value(value).unwrap();
    }
}
