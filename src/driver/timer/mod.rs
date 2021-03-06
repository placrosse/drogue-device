use crate::actor::Configurable;
use crate::alloc::{alloc, Box};
use crate::domain::time::duration::{Duration, Milliseconds};
use crate::hal::timer::Timer as HalTimer;
use crate::prelude::*;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use cortex_m::interrupt::Nr;

#[derive(Copy, Clone, Debug)]
pub struct Delay<DUR: Duration + Into<Milliseconds>>(pub DUR);

pub trait Schedulable {
    fn run(&self);
    fn get_expiration(&self) -> Milliseconds;
    fn set_expiration(&mut self, expiration: Milliseconds);
}

#[derive(Clone)]
pub struct Schedule<A, DUR, E>
where
    A: Actor + NotifyHandler<E> + 'static,
    DUR: Duration + Into<Milliseconds>,
    E: Clone + 'static,
{
    delay: DUR,
    event: E,
    address: Address<A>,
}

impl<A, DUR, E> Schedule<A, DUR, E>
where
    A: Actor + NotifyHandler<E> + 'static,
    DUR: Duration + Into<Milliseconds>,
    E: Clone + 'static,
{
    pub fn new(delay: DUR, event: E, address: Address<A>) -> Self {
        Self {
            delay,
            event,
            address,
        }
    }
}

pub struct Shared {
    current_deadline: RefCell<Option<Milliseconds>>,
    delay_deadlines: RefCell<[Option<DelayDeadline>; 16]>,
    schedule_deadlines: RefCell<[Option<Box<dyn Schedulable>>; 16]>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            current_deadline: RefCell::new(None),
            delay_deadlines: RefCell::new(Default::default()),
            schedule_deadlines: RefCell::new(Default::default()),
        }
    }

    fn has_expired(&self, index: usize) -> bool {
        let expired = self.delay_deadlines.borrow()[index]
            .as_ref()
            .unwrap()
            .expiration
            == Milliseconds(0u32);
        if expired {
            self.delay_deadlines.borrow_mut()[index].take();
        }
        expired
    }

    fn register_waker(&self, index: usize, waker: Waker) {
        self.delay_deadlines.borrow_mut()[index]
            .as_mut()
            .unwrap()
            .waker
            .replace(waker);
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Timer<T: HalTimer + 'static> {
    actor: InterruptContext<TimerActor<T>>,
    shared: Shared,
}

impl<T: HalTimer> Timer<T> {
    pub fn new<IRQ: Nr>(timer: T, irq: IRQ) -> Self {
        Self {
            actor: InterruptContext::new(TimerActor::new(timer), irq).with_name("timer"),
            shared: Shared::default(),
        }
    }
}

impl<D: Device, T: HalTimer> Package<D, TimerActor<T>> for Timer<T> {
    fn mount(
        &'static self,
        bus_address: Address<EventBus<D>>,
        supervisor: &mut Supervisor,
    ) -> Address<TimerActor<T>> {
        let addr = self.actor.mount(supervisor);
        self.actor.configure(&self.shared);
        addr
    }
}

pub struct TimerActor<T: HalTimer> {
    timer: T,
    shared: Option<&'static Shared>,
}

impl<T: HalTimer> Configurable for TimerActor<T> {
    type Configuration = Shared;

    fn configure(&mut self, config: &'static Self::Configuration) {
        self.shared.replace(config);
    }
}

impl<T: HalTimer> TimerActor<T> {
    pub fn new(timer: T) -> Self {
        Self {
            timer,
            shared: None,
        }
    }
}

impl<T: HalTimer> Actor for TimerActor<T> {}

impl<T, DUR> RequestHandler<Delay<DUR>> for TimerActor<T>
where
    T: HalTimer,
    DUR: Duration + Into<Milliseconds>,
{
    type Response = ();

    fn on_request(mut self, message: Delay<DUR>) -> Response<Self, Self::Response> {
        let ms: Milliseconds = message.0.into();

        if let Some((index, slot)) = self
            .shared
            .unwrap()
            .delay_deadlines
            .borrow_mut()
            .iter_mut()
            .enumerate()
            .find(|e| matches!(e, (_, None)))
        {
            self.shared.unwrap().delay_deadlines.borrow_mut()[index]
                .replace(DelayDeadline::new(ms));
            if let Some(current_deadline) = &*self.shared.unwrap().current_deadline.borrow() {
                if *current_deadline > ms {
                    self.shared
                        .unwrap()
                        .current_deadline
                        .borrow_mut()
                        .replace(ms);
                    //log::info!("start shorter timer for {:?}", ms);
                    self.timer.start(ms);
                } else {
                    //log::info!("timer already running for {:?}", current_deadline );
                }
            } else {
                self.shared
                    .unwrap()
                    .current_deadline
                    .borrow_mut()
                    .replace(ms);
                //log::info!("start new timer for {:?}", ms);
                self.timer.start(ms);
            }
            let future = DelayFuture::new(index, self.shared.as_ref().unwrap());
            Response::immediate_future(self, future)
        } else {
            Response::immediate(self, ())
        }
    }
}

impl<T, E, A, DUR> NotifyHandler<Schedule<A, DUR, E>> for TimerActor<T>
where
    T: HalTimer + 'static,
    E: Clone + 'static,
    A: Actor + NotifyHandler<E> + 'static,
    DUR: Duration + Into<Milliseconds> + 'static,
{
    fn on_notify(mut self, message: Schedule<A, DUR, E>) -> Completion<Self> {
        let ms: Milliseconds = message.delay.into();
        // log::info!("schedule request {:?}", ms);
        let mut deadlines = self.shared.unwrap().schedule_deadlines.borrow_mut();
        let mut current_deadline = self.shared.unwrap().current_deadline.borrow_mut();

        if let Some((index, slot)) = deadlines
            .iter_mut()
            .enumerate()
            .find(|e| matches!(e, (_, None)))
        {
            deadlines[index].replace(Box::new(alloc(ScheduleDeadline::new(ms, message)).unwrap()));
            if let Some(current) = &*current_deadline {
                if *current > ms {
                    current_deadline.replace(ms);
                    self.timer.start(ms);
                } else {
                    //log::info!("timer already running for {:?}", current_deadline );
                }
            } else {
                current_deadline.replace(ms);
                //log::info!("start new timer for {:?}", ms);
                self.timer.start(ms);
            }
        }
        Completion::immediate(self)
    }
}

impl<T: HalTimer> Interrupt for TimerActor<T> {
    fn on_interrupt(&mut self) {
        self.timer.clear_update_interrupt_flag();
        let expired = self.shared.unwrap().current_deadline.borrow().unwrap();

        let mut delay_deadlines = self.shared.unwrap().delay_deadlines.borrow_mut();

        let mut next_deadline = None;
        //log::info!("timer expired! {:?}", expired);
        for slot in delay_deadlines.iter_mut() {
            if let Some(deadline) = slot {
                if deadline.expiration >= expired {
                    deadline.expiration = deadline.expiration - expired;
                } else {
                    deadline.expiration = Milliseconds(0u32);
                }

                if deadline.expiration == Milliseconds(0u32) {
                    deadline.waker.take().unwrap().wake();
                } else {
                    match next_deadline {
                        None => {
                            next_deadline.replace(deadline.expiration);
                        }
                        Some(soonest) if soonest > deadline.expiration => {
                            next_deadline.replace(deadline.expiration);
                        }
                        _ => { /* ignore */ }
                    }
                }
            }
        }

        let mut schedule_deadlines = self.shared.unwrap().schedule_deadlines.borrow_mut();

        for slot in schedule_deadlines.iter_mut() {
            if let Some(deadline) = slot {
                let expiration = deadline.get_expiration();
                if expiration >= expired {
                    deadline.set_expiration(expiration - expired);
                } else {
                    deadline.set_expiration(Milliseconds(0u32));
                }

                if deadline.get_expiration() == Milliseconds(0u32) {
                    deadline.run();
                    slot.take();
                } else {
                    match next_deadline {
                        None => {
                            next_deadline.replace(deadline.get_expiration());
                        }
                        Some(soonest) if soonest > deadline.get_expiration() => {
                            next_deadline.replace(deadline.get_expiration());
                        }
                        _ => { /* ignore */ }
                    }
                }
            }
        }

        let mut current_deadline = self.shared.unwrap().current_deadline.borrow_mut();
        //log::info!("next deadline {:?}", next_deadline );

        if let Some(next_deadline) = next_deadline {
            if next_deadline > Milliseconds(0u32) {
                current_deadline.replace(next_deadline);
                self.timer.start(next_deadline);
            } else {
                current_deadline.take();
            }
        } else {
            current_deadline.take();
        }
    }
}

impl<T: HalTimer + 'static> Address<TimerActor<T>> {
    pub async fn delay<DUR: Duration + Into<Milliseconds> + 'static>(&self, duration: DUR) {
        self.request(Delay(duration)).await
    }

    pub fn schedule<
        DUR: Duration + Into<Milliseconds> + 'static,
        E: Clone + 'static,
        A: Actor + NotifyHandler<E>,
    >(
        &self,
        delay: DUR,
        event: E,
        address: Address<A>,
    ) {
        self.notify(Schedule::new(delay, event, address));
    }
}

struct DelayDeadline {
    expiration: Milliseconds,
    waker: Option<Waker>,
}

impl DelayDeadline {
    fn new(expiration: Milliseconds) -> Self {
        Self {
            expiration,
            waker: None,
        }
    }
}

pub struct ScheduleDeadline<
    A: Actor + NotifyHandler<E> + 'static,
    DUR: Duration + Into<Milliseconds>,
    E: Clone + 'static,
> {
    expiration: Milliseconds,
    schedule: Schedule<A, DUR, E>,
}

impl<
        A: Actor + NotifyHandler<E> + 'static,
        DUR: Duration + Into<Milliseconds>,
        E: Clone + 'static,
    > Schedulable for ScheduleDeadline<A, DUR, E>
{
    fn run(&self) {
        self.schedule.address.notify(self.schedule.event.clone());
    }

    fn set_expiration(&mut self, expiration: Milliseconds) {
        self.expiration = expiration;
    }

    fn get_expiration(&self) -> Milliseconds {
        self.expiration
    }
}

impl<
        A: Actor + NotifyHandler<E> + 'static,
        DUR: Duration + Into<Milliseconds>,
        E: Clone + 'static,
    > ScheduleDeadline<A, DUR, E>
{
    fn new(expiration: Milliseconds, schedule: Schedule<A, DUR, E>) -> Self {
        Self {
            expiration,
            schedule,
        }
    }
}

struct DelayFuture {
    index: usize,
    shared: &'static Shared,
    expired: bool,
}

impl DelayFuture {
    fn new(index: usize, shared: &'static Shared) -> Self {
        Self {
            index,
            shared,
            expired: false,
        }
    }

    fn has_expired(&mut self) -> bool {
        if !self.expired {
            // critical section to avoid being trampled by the timer's own IRQ
            self.expired = cortex_m::interrupt::free(|cs| self.shared.has_expired(self.index))
        }

        self.expired
    }

    fn register_waker(&self, waker: &Waker) {
        //unsafe {
        //(&mut **self.timer.get()).register_waker(self.index, waker.clone());
        //}
        self.shared.register_waker(self.index, waker.clone());
    }
}

impl Future for DelayFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.has_expired() {
            //log::info!("delay poll - ready {}", self.index);
            Poll::Ready(())
        } else {
            //log::info!("delay poll - pending {}", self.index);
            self.register_waker(cx.waker());
            Poll::Pending
        }
    }
}
