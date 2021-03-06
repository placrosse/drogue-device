use crate::bind::Bind;
use crate::domain::time::duration::Milliseconds;
use crate::driver::led::simple::Switchable;
use crate::driver::timer::TimerActor;
use crate::hal::timer::Timer as HalTimer;
use crate::prelude::*;

pub struct Blinker<S, T>
where
    S: Switchable + 'static,
    T: HalTimer + 'static,
{
    led: Option<Address<S>>,
    timer: Option<Address<TimerActor<T>>>,
    delay: Milliseconds,
    address: Option<Address<Self>>,
}

impl<S, T> Blinker<S, T>
where
    S: Switchable,
    T: HalTimer,
{
    pub fn new<DUR: Into<Milliseconds>>(delay: DUR) -> Self {
        Self {
            led: None,
            timer: None,
            delay: delay.into(),
            address: None,
        }
    }
}

impl<S, T> Bind<S> for Blinker<S, T>
where
    S: Switchable,
    T: HalTimer,
{
    fn on_bind(&mut self, address: Address<S>) {
        self.led.replace(address);
    }
}

impl<S, T> Bind<TimerActor<T>> for Blinker<S, T>
where
    S: Switchable,
    T: HalTimer,
{
    fn on_bind(&mut self, address: Address<TimerActor<T>>) {
        self.timer.replace(address);
    }
}

impl<S, T> Actor for Blinker<S, T>
where
    S: Switchable,
    T: HalTimer,
{
    fn on_mount(&mut self, address: Address<Self>)
    where
        Self: Sized,
    {
        self.address.replace(address);
    }

    fn on_start(self) -> Completion<Self> {
        self.timer.unwrap().schedule(
            self.delay,
            State::On,
            self.address.unwrap(),
        );
        Completion::immediate(self)
    }
}

#[derive(Copy, Clone, Debug)]
enum State {
    On,
    Off,
}

impl<S, T> NotifyHandler<State> for Blinker<S, T>
where
    S: Switchable,
    T: HalTimer,
{
    fn on_notify(self, message: State) -> Completion<Self> {
        match message {
            State::On => {
                self.led.unwrap().turn_on();
                self.timer.unwrap().schedule(
                    self.delay,
                    State::Off,
                    self.address.unwrap(),
                );
            }
            State::Off => {
                self.led.unwrap().turn_off();
                self.timer.unwrap().schedule(
                    self.delay,
                    State::On,
                    self.address.unwrap(),
                );
            }
        }
        Completion::immediate(self)
    }
}

pub struct AdjustDelay(Milliseconds);

impl<S, T> NotifyHandler<AdjustDelay> for Blinker<S, T>
where
    S: Switchable,
    T: HalTimer,
{
    fn on_notify(mut self, message: AdjustDelay) -> Completion<Self> {
        self.delay = message.0;
        Completion::immediate(self)
    }
}

impl<S, T> Address<Blinker<S, T>>
where
    Self: 'static,
    S: Switchable,
    T: HalTimer,
{
    pub fn adjust_delay(&self, delay: Milliseconds) {
        self.notify(AdjustDelay(delay))
    }
}
