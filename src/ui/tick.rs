//! Управление tick-колбэком GTK: настоящий сон вместо пустого `return`.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::TickCallbackId;

/// Хранит id tick-колбэка и умеет снимать/вешать его заново.
/// `add_tick_callback` сам по себе привязан к frame clock: если просто
/// возвращать Continue, GTK всё равно будит поток 60 раз в секунду.
pub struct TickControl {
    id: RefCell<Option<TickCallbackId>>,
    installer: Box<dyn Fn() -> TickCallbackId>,
}

impl TickControl {
    pub fn new(installer: impl Fn() -> TickCallbackId + 'static) -> Rc<Self> {
        Rc::new(Self {
            id: RefCell::new(None),
            installer: Box::new(installer),
        })
    }

    pub fn wake(&self) {
        if self.id.borrow().is_none() {
            *self.id.borrow_mut() = Some((self.installer)());
        }
    }

    pub fn sleep(&self) {
        if let Some(id) = self.id.borrow_mut().take() {
            id.remove();
        }
    }

    pub fn is_active(&self) -> bool {
        self.id.borrow().is_some()
    }
}
