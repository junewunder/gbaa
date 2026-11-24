// Typed pointers from the names files (native/lift-typed-access;
// `codegen::native::fields`): what the `t` module's accessors and the
// accesses printed through them need. Written into a lifted crate only
// when its lift has types. A `Ref` here is only a typed address: `get`
// and `set` are `ld*` and `st*` at it, whatever it points at, but `get`
// checks it is aligned as its type is (native/lift-aligned-loads).

/// The `t` module's name for a typed pointer.
pub type Ptr<T> = Ref<T>;

impl<T> Ref<T> {
    pub const fn at(a: u32) -> Ref<T> {
        Ref(a, std::marker::PhantomData)
    }

    /// A field `off` bytes in.
    #[inline(always)]
    pub const fn field<F>(self, off: u32) -> Ref<F> {
        Ref(self.0.wrapping_add(off), std::marker::PhantomData)
    }
}

/// A type the guest lays out: its size there.
pub trait Guest {
    const SIZE: u32;
}
impl Guest for u8 {
    const SIZE: u32 = 1;
}
impl Guest for u16 {
    const SIZE: u32 = 2;
}
impl Guest for u32 {
    const SIZE: u32 = 4;
}
impl<T> Guest for Ref<T> {
    const SIZE: u32 = 4;
}
impl<T: Guest, const N: usize> Guest for [T; N] {
    const SIZE: u32 = T::SIZE * N as u32;
}

impl<T: Guest, const N: usize> Ref<[T; N]> {
    /// `&a[i]`.
    #[inline(always)]
    pub fn idx(self, i: u32) -> Ref<T> {
        Ref(self.0.wrapping_add(i.wrapping_mul(T::SIZE)), std::marker::PhantomData)
    }
}

impl<T: Guest> Ref<T> {
    /// `&p[i]`: the `i`th `T` on from this one.
    #[inline(always)]
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, i: u32) -> Ref<T> {
        Ref(self.0.wrapping_add(i.wrapping_mul(T::SIZE)), std::marker::PhantomData)
    }
}

/// A value loaded or stored whole: its bytes.
pub trait Word {
    const BYTES: u32;
}
impl Word for u8 {
    const BYTES: u32 = 1;
}
impl Word for u16 {
    const BYTES: u32 = 2;
}
impl Word for u32 {
    const BYTES: u32 = 4;
}
impl<T> Word for Ref<T> {
    const BYTES: u32 = 4;
}

#[cold]
#[track_caller]
fn misaligned(a: u32, n: u32) -> ! {
    panic!("typed pointer {a:#010x} to {n} bytes isn't aligned to {n}: a load its C type doesn't explain")
}

/// A bitfield (`T x : N` in C): `N` bits from bit `LO` of the `T` its
/// accessor points at, which loads and stores as that `T`.
pub struct Bits<T, const LO: u32, const N: u32>(pub T);
impl<T: Word, const LO: u32, const N: u32> Word for Bits<T, LO, N> {
    const BYTES: u32 = T::BYTES;
}

impl Game {
    /// `*p`, as `ld8`, `ld16` or `ld32` loads it, `p` checked aligned as
    /// its C type is: the lift prints a typed load without the rotate a
    /// misaligned `ldr` or `ldrh` would give it (native/lift-aligned-loads).
    #[inline(always)]
    #[track_caller]
    pub fn get<T: Word>(&mut self, p: Ref<T>) -> u32 {
        if p.0 & (T::BYTES - 1) != 0 {
            misaligned(p.0, T::BYTES);
        }
        match T::BYTES {
            1 => ld8(self, p.0),
            2 => ld16(self, p.0),
            _ => ld32(self, p.0),
        }
    }

    /// `*p = v`, as `st8`, `st16` or `st32` stores it.
    #[inline(always)]
    pub fn set<T: Word>(&mut self, p: Ref<T>, v: u32) {
        match T::BYTES {
            1 => st8(self, p.0, v),
            2 => st16(self, p.0 & !1, v),
            _ => st32(self, p.0 & !3, v),
        }
    }

    /// `p`'s bitfield set to `v` (`x.f = v;`, native/lift-bitfields): a
    /// load of the bytes that hold it, then a store of them with the
    /// field's bits replaced by `v`'s low ones, as the code it stands for
    /// does.
    #[inline(always)]
    pub fn set_bits<T: Word, const LO: u32, const N: u32>(&mut self, p: Ref<Bits<T, LO, N>>, v: u32) {
        let old = self.get(p);
        let mask = (u32::MAX >> (32 - N)) << LO;
        self.set(p, (old & !mask) | ((v << LO) & mask));
    }
}
