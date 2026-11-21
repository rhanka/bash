//! Gate 4: no leak over a long, fixed-seed mix of operations on every container type.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;

const OPS: usize = 30_000;
const SLOTS: usize = 6;

#[test]
fn mixed_operations_leak_nothing() {
    start();
    {
        let parent = root(61, u64::MAX);
        // Tight enough that growth is refused now and then.
        let child = parent.child(LimitId(62), 24 * 1024).unwrap();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut vecs: [Option<KVec<u64>>; SLOTS] = Default::default();
        let mut bytes: [Option<KBytes>; SLOTS] = Default::default();
        let mut strings: [Option<KString>; SLOTS] = Default::default();
        let mut boxes: [Option<KBox<[u8; 64]>>; SLOTS] = Default::default();
        let mut rcs: [Option<KRc<[u8; 64]>>; SLOTS] = Default::default();
        let mut maps: [Option<KMap<u32, KString>>; SLOTS] = Default::default();
        let (mut refused, mut done) = (0u32, 0u32);

        for step in 0..OPS {
            let s = rng.below(SLOTS as u64) as usize;
            let r = rng.next();
            let res: MemResult<()> = match rng.below(6) {
                0 => {
                    let v = vecs[s].get_or_insert_with(|| KVec::new_in(&child));
                    match r % 8 {
                        0..=3 => v.try_push(r),
                        4 => v.try_insert(v.len() / 2, r),
                        5 => {
                            if !v.is_empty() {
                                v.remove(v.len() / 3);
                            }
                            Ok(())
                        }
                        6 => v.try_extend_from_slice(&[1, 2, 3, r]),
                        _ => {
                            v.truncate(v.len() / 2);
                            v.shrink_to_fit();
                            Ok(())
                        }
                    }
                }
                1 => {
                    let v = bytes[s].get_or_insert_with(|| KVec::new_in(&child));
                    match r % 6 {
                        0..=2 => v.try_resize(v.len() + (r % 200) as usize, r as u8),
                        3 => v.try_reserve_exact((r % 100) as usize),
                        4 => {
                            v.clear();
                            Ok(())
                        }
                        _ => bytes[s].as_ref().unwrap().try_clone().map(|c| {
                            drop(c);
                        }),
                    }
                }
                2 => {
                    let t = strings[s].get_or_insert_with(|| KString::new_in(&child));
                    match r % 5 {
                        0 | 1 => t.try_push_str("ab\u{e9}\u{4e2d}"),
                        2 => t.try_push('\u{1f600}'),
                        3 => {
                            t.clear();
                            Ok(())
                        }
                        _ => strings[s].as_ref().unwrap().try_clone().map(drop),
                    }
                }
                3 => {
                    if boxes[s].is_some() && r % 2 == 0 {
                        boxes[s] = None;
                        Ok(())
                    } else {
                        match KBox::try_new_in([r as u8; 64], &child) {
                            Ok(b) => {
                                boxes[s] = Some(b);
                                Ok(())
                            }
                            Err((v, e)) => {
                                assert_eq!(v[0], r as u8);
                                Err(e)
                            }
                        }
                    }
                }
                4 => {
                    let other = ((r >> 8) % SLOTS as u64) as usize;
                    match r % 4 {
                        0 => match KRc::try_new_in([r as u8; 64], &child) {
                            Ok(x) => {
                                rcs[s] = Some(x);
                                Ok(())
                            }
                            Err((_, e)) => Err(e),
                        },
                        1 => {
                            rcs[other] = rcs[s].clone();
                            Ok(())
                        }
                        2 => {
                            if let Some(x) = rcs[s].take() {
                                if let Ok(v) = KRc::try_unwrap(x) {
                                    assert_eq!(v[1], v[0]);
                                }
                            }
                            Ok(())
                        }
                        _ => {
                            rcs[s] = None;
                            Ok(())
                        }
                    }
                }
                _ => {
                    let m = maps[s].get_or_insert_with(|| KMap::new_in(&child));
                    let key = (r >> 16) as u32 % 64;
                    match r % 4 {
                        0 | 1 => KString::try_from_str_in("value", &child).and_then(|t| m.try_insert(key, t).map(drop)),
                        2 => {
                            m.remove(&key);
                            Ok(())
                        }
                        _ => maps[s].as_ref().unwrap().try_clone().map(drop),
                    }
                }
            };
            match res {
                Ok(()) => done += 1,
                Err(MemError::Limit(id)) => {
                    assert_eq!(id, LimitId(62));
                    refused += 1;
                }
                Err(e) => panic!("step {step}: unexpected {e:?}"),
            }
            assert_eq!(child.used(), parent.used());
            assert!(child.used() <= 24 * 1024);
            if r % 97 == 0 {
                // Keep the pressure around the cap: drop a whole slot of some type.
                match r % 6 {
                    0 => vecs[s] = None,
                    1 => bytes[s] = None,
                    2 => strings[s] = None,
                    3 => maps[s] = None,
                    4 => boxes[s] = None,
                    _ => rcs[s] = None,
                }
            }
        }
        assert!(refused > 0 && done > OPS as u32 / 2, "the run exercises both outcomes ({refused} refused, {done} done)");
        drop((vecs, bytes, strings, boxes, rcs, maps));
        assert_eq!(child.used(), 0);
        assert_eq!(parent.used(), 0);
    }
}
