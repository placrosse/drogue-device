use heapless::{
    Vec,
    consts::*,
};

use crate::actor::{Actor, ActorContext};
use core::task::{Poll, Context, Waker, RawWaker, RawWakerVTable};
use core::sync::atomic::{AtomicU8, Ordering};


pub enum ActorState {
    IDLE = 0,
    WAITING = 1,
    READY = 2,
    UNKNOWN = 127,
}

impl Into<u8> for ActorState {
    fn into(self) -> u8 {
        self as u8
    }
}

pub struct Supervised {
    actor: &'static dyn ActiveActor,
    state: AtomicU8,
}

impl Supervised {
    fn new<A: ActiveActor>(actor: &'static A) -> Self {
        Self {
            actor,
            state: AtomicU8::new(ActorState::IDLE.into()),
        }
    }

    fn get_state_flag_handle(&self) -> *const () {
        &self.state as *const _ as *const ()
    }

    fn is_idle(&self) -> bool {
        self.state.load(Ordering::Release) == ActorState::IDLE.into()
    }

    fn signal_idle(&self) {
        self.state.store(ActorState::IDLE.into(), Ordering::Acquire)
    }

    fn is_waiting(&self) -> bool {
        self.state.load(Ordering::Release) == ActorState::WAITING.into()
    }

    fn signal_waiting(&self) {
        self.state.store(ActorState::WAITING.into(), Ordering::Acquire)
    }

    fn is_ready(&self) -> bool {
        self.state.load(Ordering::Release) == ActorState::READY.into()
    }

    fn signal_ready(&self) {
        self.state.store(ActorState::READY.into(), Ordering::Acquire)
    }

    fn poll(&mut self) -> bool {
        if self.is_ready() {
            self.signal_idle();
            match self.actor.do_poll(self.get_state_flag_handle()) {
                Poll::Ready(_) => {
                    self.signal_idle()
                }
                Poll::Pending => {
                    self.signal_waiting()
                }
            }
            true
        } else {
            false
        }
    }
}

pub trait ActiveActor {
    fn do_poll(&self, state_flag_handle: *const ()) -> Poll<()>;
}

impl<A: Actor> ActiveActor for ActorContext<A> {
    fn do_poll(&self, state_flag_handle: *const ()) -> Poll<()> {
        let mut is_waiting = false;
        unsafe {
            let raw_waker = RawWaker::new(state_flag_handle, &VTABLE);
            let waker = Waker::from_raw(raw_waker);
            let mut cx = Context::from_waker(&waker);
            for item in (&mut *self.items.get()).iter_mut() {
                let result = item.poll(&mut cx);
                match result {
                    Poll::Ready(_) => {}
                    Poll::Pending => {
                        is_waiting = true
                    }
                }
            }
        }

        if is_waiting {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

pub struct Supervisor {
    actors: Vec<Supervised, U16>
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            actors: Vec::new()
        }
    }

    pub fn add<S: ActiveActor>(&mut self, actor: &'static S) {
        self.actors.push(Supervised::new(actor));
    }

    pub fn run_until_quiescence(&mut self) {
        let mut run_again = false;
        while run_again {
            run_again = false;
            for actor in self.actors.iter_mut().filter(|e| !e.is_idle()) {
                if actor.poll() {
                    run_again = true
                }
            }
        }
    }

    pub fn run_forever(&mut self) -> ! {
        loop {
            self.run_until_quiescence();
            // WFI
        }
    }
}

// NOTE `*const ()` is &AtomicU8
static VTABLE: RawWakerVTable = {
    unsafe fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    unsafe fn wake(p: *const ()) {
        wake_by_ref(p)
    }

    unsafe fn wake_by_ref(p: *const ()) {
        (*(p as *const AtomicU8)).store(ActorState::READY.into(), Ordering::Release);
    }

    unsafe fn drop(_: *const ()) {}

    RawWakerVTable::new(clone, wake, wake_by_ref, drop)
};