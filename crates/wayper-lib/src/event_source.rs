//! Custom event source for calloop, so we can wakeup the draw function anytime it is
//! needed

use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop;

pub struct DrawSource {
    timer: calloop::timer::Timer,
    draw_ping_receiver: calloop::ping::PingSource,
}

impl DrawSource {
    pub fn from_duration(duration: Duration) -> std::io::Result<(Self, calloop::ping::Ping)> {
        let timer = calloop::timer::Timer::from_duration(duration);

        let (draw_ping_sender, draw_ping_receiver) = calloop::ping::make_ping()?;

        Ok((
            Self {
                timer,
                draw_ping_receiver,
            },
            draw_ping_sender,
        ))
    }
}

impl calloop::EventSource for DrawSource {
    type Event = DrawSourceEvent;

    type Metadata = ();

    type Ret = calloop::timer::TimeoutAction;

    type Error = color_eyre::eyre::Error;

    fn process_events<F>(
        &mut self,
        readiness: calloop::Readiness,
        token: calloop::Token,
        mut callback: F,
    ) -> Result<calloop::PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.draw_ping_receiver
            .process_events(readiness, token, |_, _| {
                callback(DrawSourceEvent::PingTrigger, &mut ());
            })?;

        self.timer
            .process_events(readiness, token, |previous_deadline, metadata| {
                callback(DrawSourceEvent::TimerTrigger(previous_deadline), metadata)
            })?;

        Ok(calloop::PostAction::Continue)
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.timer.register(poll, token_factory)?;
        self.draw_ping_receiver.register(poll, token_factory)?;

        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.timer.reregister(poll, token_factory)?;
        self.draw_ping_receiver.reregister(poll, token_factory)?;

        Ok(())
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        self.timer.unregister(poll)?;
        self.draw_ping_receiver.unregister(poll)?;

        Ok(())
    }
}

pub enum DrawSourceEvent {
    TimerTrigger(Instant),
    PingTrigger,
}

impl DrawSourceEvent {
    /// Use this function to calculate the next timer instance if needed
    pub fn get_last_deadline(&self) -> Instant {
        match self {
            DrawSourceEvent::TimerTrigger(instant) => *instant,
            DrawSourceEvent::PingTrigger => Instant::now(),
        }
    }
}
