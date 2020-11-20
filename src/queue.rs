use crate::{
    alloc::{AllocRef, Layout},
    unprotected, Atomic, CachePadded, Shared, Shield,
};
use core::{
    cell::UnsafeCell,
    mem::{self, MaybeUninit},
    ptr,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
};

const BUFFER_SIZE: usize = 256;

pub struct Queue<T, A>
where
    A: AllocRef,
{
    head: CachePadded<Atomic<Node<T>>>,
    tail: CachePadded<Atomic<Node<T>>>,
    allocator: A,
}

impl<T, A> Queue<T, A>
where
    A: AllocRef,
{
    pub fn new(allocator: A) -> Self {
        let sentinel = Node::new(None, 0, &allocator);

        Self {
            head: CachePadded::new(Atomic::new(sentinel)),
            tail: CachePadded::new(Atomic::new(sentinel)),
            allocator,
        }
    }

    fn cas_head<'a, S>(
        &self,
        current: Shared<'_, Node<T>>,
        next: Shared<'_, Node<T>>,
        shield: &S,
    ) -> bool
    where
        S: Shield<'a>,
    {
        self.head
            .compare_and_swap(current, next, Ordering::SeqCst, shield)
            == current
    }

    fn cas_tail<'a, S>(&self, current: Shared<'_, Node<T>>, next: Shared<'_, Node<T>>, shield: &S)
    where
        S: Shield<'a>,
    {
        self.tail
            .compare_and_swap(current, next, Ordering::SeqCst, shield);
    }

    pub fn push<'a, S>(&self, value: T, shield: &S)
    where
        S: Shield<'a>,
    {
        loop {
            let ltail = self.tail.load(Ordering::SeqCst, shield);
            let ltail_ref = unsafe { ltail.as_ref_unchecked() };
            let idx = ltail_ref.enq_allocated.fetch_add(1, Ordering::SeqCst);

            if idx > BUFFER_SIZE - 1 {
                if ltail != self.tail.load(Ordering::SeqCst, shield) {
                    continue;
                }

                let lnext = ltail_ref.next.load(Ordering::SeqCst, shield);

                if lnext.is_null() {
                    let new_node =
                        Node::new(Some(unsafe { ptr::read(&value) }), 1, &self.allocator);

                    if ltail_ref.cas_next(Shared::null(), new_node, shield) {
                        self.cas_tail(ltail, new_node, shield);
                        mem::forget(value);
                        return;
                    } else {
                        Node::destroy(new_node, &self.allocator);
                    }
                } else {
                    self.cas_tail(ltail, lnext, shield);
                }

                continue;
            }

            unsafe {
                ltail_ref.items[idx].write(value);
                let idx = idx as isize;
                while ltail_ref
                    .enq_committed
                    .compare_and_swap(idx - 1, idx, Ordering::SeqCst)
                    != idx - 1
                {}
                return;
            }
        }
    }

    pub fn pop_if<'a, 'shield, F, S>(&self, f: F, shield: &'shield S) -> Option<Shared<'shield, T>>
    where
        F: Fn(&T) -> bool,
        S: Shield<'a>,
        T: 'a,
    {
        loop {
            let lhead = self.head.load(Ordering::SeqCst, shield);
            let lhead_ref = unsafe { lhead.as_ref_unchecked() };
            let idx = lhead_ref.deqidx.load(Ordering::SeqCst);

            if idx > BUFFER_SIZE - 1 {
                let lnext = lhead_ref.next.load(Ordering::SeqCst, shield);

                if lnext.is_null() {
                    break;
                }

                if self.cas_head(lhead, lnext, shield) {
                    let allocator = self.allocator.clone();
                    shield.retire(move || Node::destroy(lhead, &allocator));
                }

                continue;
            }

            if idx as isize > lhead_ref.enq_committed.load(Ordering::SeqCst) {
                break;
            }

            let entry = &lhead_ref.items[idx];
            let item = unsafe { entry.shared() };

            if !f(unsafe { item.as_ref_unchecked() }) {
                return None;
            } else if lhead_ref
                .deqidx
                .compare_and_swap(idx, idx + 1, Ordering::SeqCst)
                != idx
            {
                continue;
            }

            return Some(item);
        }

        None
    }
}

impl<T, A> Drop for Queue<T, A>
where
    A: AllocRef,
{
    fn drop(&mut self) {
        let shield = unsafe { unprotected() };
        while let Some(_) = self.pop_if(|_| true, shield) {}
    }
}

unsafe impl<T, A> Send for Queue<T, A>
where
    T: Send,
    A: Send + AllocRef,
{
}

unsafe impl<T, A> Sync for Queue<T, A>
where
    T: Send + Sync,
    A: Send + Sync + AllocRef,
{
}

struct Node<T> {
    enq_allocated: CachePadded<AtomicUsize>,
    enq_committed: CachePadded<AtomicIsize>,
    deqidx: CachePadded<AtomicUsize>,
    next: CachePadded<Atomic<Self>>,
    items: [Entry<T>; BUFFER_SIZE],
}

impl<T> Node<T> {
    fn new<'a, A>(maybe_item: Option<T>, enqidx: usize, allocator: &A) -> Shared<'a, Self>
    where
        A: AllocRef,
    {
        let first_entry = Entry::new();

        if let Some(item) = maybe_item {
            unsafe {
                first_entry.write(item);
            }
        }

        let node = Self {
            enq_allocated: CachePadded::new(AtomicUsize::new(enqidx)),
            enq_committed: CachePadded::new(AtomicIsize::new(enqidx as isize - 1)),
            deqidx: CachePadded::new(AtomicUsize::new(0)),
            next: CachePadded::new(Atomic::null()),
            items: [
                first_entry,
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
                Entry::new(),
            ],
        };

        let layout = Layout::of::<Self>();

        unsafe {
            let ptr = allocator.alloc(layout) as *mut Self;
            ptr::write(ptr, node);
            ptr
        }
    }

    fn destroy<'a, A>(instance: Shared<'a, Self>, allocator: &A)
    where
        A: AllocRef,
    {
        let layout = Layout::of::<Self>();
        let ptr = instance.as_ptr();

        unsafe {
            ptr::drop_in_place(ptr);
            allocator.dealloc(ptr as *mut u8, layout);
        }
    }

    fn cas_next<'a, S>(&self, current: Shared<'_, Self>, next: Shared<'_, Self>, shield: &S) -> bool
    where
        S: Shield<'a>,
    {
        self.next
            .compare_and_swap(current, next, Ordering::SeqCst, shield)
            == current
    }
}

struct Entry<T> {
    data: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Entry<T> {
    fn new() -> Self {
        Self {
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    unsafe fn write(&self, item: T) {
        let data_ptr = self.data.get() as *mut T;
        ptr::write(data_ptr, item);
    }

    unsafe fn shared<'a>(&self) -> Shared<'a, T> {
        let data_ptr = self.data.get() as *mut T;
        Shared::from_ptr(data_ptr)
    }
}

#[cfg(test)]
mod tests {
    use super::Queue;
    use crate::alloc::GlobalAllocator;
    use crate::Collector;

    macro_rules! matches {
        ($expression:expr, $( $pattern:pat )|+ $( if $guard: expr )?) => {
            match $expression {
                $( $pattern )|+ $( if $guard )? => true,
                _ => false
            }
        }
    }

    #[test]
    fn push_pop_check() {
        let collector = Collector::new();
        let shield = collector.thin_shield();
        let queue = Queue::new(GlobalAllocator);
        queue.push(5, &shield);
        queue.push(10, &shield);
        assert!(matches!(queue.pop_if(|x| *x == 10, &shield), None));
        assert!(matches!(queue.pop_if(|x| *x == 5, &shield), Some(_)));
        assert!(matches!(queue.pop_if(|x| *x == 5, &shield), None));
        assert!(matches!(queue.pop_if(|x| *x == 10, &shield), Some(_)));
    }
}
