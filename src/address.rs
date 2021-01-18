use crate::actor::{Actor, ActorContext};
use core::cell::UnsafeCell;
use crate::handler::{AskHandler, TellHandler};

pub struct Address<A: Actor> {
    actor: UnsafeCell<*const ActorContext<A>>,
}

impl<A:Actor> Clone for Address<A> {
    fn clone(&self) -> Self {
        Self {
            actor: unsafe { UnsafeCell::new( &**self.actor.get() ) }
        }
    }
}

// TODO critical sections around ask/tell
impl<A: Actor> Address<A> {
    pub(crate) fn new(actor: &ActorContext<A>) -> Self {
        Self {
            actor: UnsafeCell::new(actor),
        }
    }

    pub fn tell<M>(&self, message: M)
        where A: TellHandler<M> + 'static,
              M: 'static
    {
        unsafe {
            (&**self.actor.get()).tell(message);
        }
    }

    pub async fn ask<M>(&self, message: M) -> <A as AskHandler<M>>::Response
        where A: AskHandler<M> + 'static,
              M: 'static
    {
        unsafe {
            (&**self.actor.get()).ask(message).await
        }
    }
}