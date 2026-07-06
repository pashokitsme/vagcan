//! rod_crack -- multithreaded brute-forcer for a .rod zlib section's corrupted
//! first-block plaintext bytes (deflate bytes 1..5 = plaintext[3..8]).
//!
//! See research/rod-labels.md and research/clb-crack/rod_struc_decode.py.
//! Reads `crack_input.bin` produced by the Python prep step:
//!   u32 plainlen | u8 d0 | 5x (u16 nset, nset bytes) | u32 taillen, tail bytes
//! Deflate stream = [d0] ++ [d1..d5 unknown] ++ tail ; zlib stream = 78 da ++ deflate.
//! Uses an incremental-Kraft dynamic-Huffman header parse as a fast filter,
//! then miniz_oxide inflate as the definitive oracle (must yield `plainlen`).

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const CLCL_ORDER: [usize; 19] = [16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1,15];

struct Bits<'a> {
    d0: u8,
    guess: &'a [u8; 5],
    tail: &'a [u8],
    byte: usize,
    nbits: u32,
    cur: u64,
}
impl<'a> Bits<'a> {
    fn new(d0: u8, guess: &'a [u8;5], tail: &'a [u8]) -> Self {
        Bits { d0, guess, tail, byte: 0, nbits: 0, cur: 0 }
    }
    #[inline]
    fn getb(&self, i: usize) -> u8 {
        if i == 0 { self.d0 }
        else if i <= 5 { self.guess[i-1] }
        else { self.tail[i-6] }
    }
    #[inline]
    fn read(&mut self, n: u32) -> u64 {
        while self.nbits < n {
            self.cur |= (self.getb(self.byte) as u64) << self.nbits;
            self.byte += 1;
            self.nbits += 8;
        }
        let v = self.cur & ((1u64 << n) - 1);
        self.cur >>= n;
        self.nbits -= n;
        v
    }
}

fn kraft_ok(lens: &[u8]) -> bool {
    let mut cnt = [0i64; 16];
    let mut maxbl = 0usize;
    for &l in lens { if l>0 { cnt[l as usize]+=1; if (l as usize)>maxbl {maxbl=l as usize;} } }
    if maxbl==0 { return true; }
    let mut left: i64 = 1;
    for bl in 1..=maxbl {
        left <<= 1;
        left -= cnt[bl];
        if left < 0 { return false; }
    }
    true
}

// canonical huffman: returns (first_code[len], first_sym_index[len], sorted symbols)
struct Huff { counts: [u16;16], symbols: Vec<u16>, maxlen: u32 }
fn build_huff(lens: &[u8]) -> Option<Huff> {
    let mut counts = [0u16;16];
    let mut maxlen = 0u32;
    for &l in lens { if l>0 { counts[l as usize]+=1; if l as u32>maxlen {maxlen=l as u32;} } }
    if maxlen==0 { return None; }
    // offsets
    let mut offs = [0u16;16];
    let mut s=0u16;
    for i in 1..16 { offs[i]=s; s+=counts[i]; }
    let mut symbols = vec![0u16; s as usize];
    for (sym,&l) in lens.iter().enumerate() {
        if l>0 { symbols[offs[l as usize] as usize]=sym as u16; offs[l as usize]+=1; }
    }
    Some(Huff{counts,symbols,maxlen})
}
#[inline]
fn decode_sym(bs:&mut Bits, h:&Huff) -> Option<u16> {
    let mut code:i32=0; let mut first:i32=0; let mut index:i32=0;
    for len in 1..=h.maxlen as usize {
        code |= bs.read(1) as i32;
        let count = h.counts[len] as i32;
        if code - first < count {
            return Some(h.symbols[(index + (code-first)) as usize]);
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    None
}

/// Full dynamic-Huffman header parse (must be BTYPE=2). Returns true if the
/// header decodes cleanly with valid Kraft on CL, lit and dist code lengths.
fn header_ok(d0:u8, guess:&[u8;5], tail:&[u8]) -> bool {
    let mut bs = Bits::new(d0, guess, tail);
    let _bfinal = bs.read(1);
    if bs.read(2) != 2 { return false; }
    let hlit = bs.read(5) as usize + 257;
    let hdist = bs.read(5) as usize + 1;
    let hclen = bs.read(4) as usize + 4;
    let mut cl = [0u8;19];
    let mut cnt = [0i64;16];
    for i in 0..hclen {
        let l = bs.read(3) as u8;
        cl[CLCL_ORDER[i]] = l;
        if l>0 {
            cnt[l as usize]+=1;
            // incremental over-subscription
            let mut left:i64=1; let mut over=false;
            for bl in 1..=7 { left<<=1; left-=cnt[bl]; if left<0 {over=true;break;} }
            if over { return false; }
        }
    }
    let clh = match build_huff(&cl) { Some(h)=>h, None=>return false };
    let n = hlit + hdist;
    let mut lengths: Vec<u8> = Vec::with_capacity(n+8);
    while lengths.len() < n {
        let sym = match decode_sym(&mut bs, &clh) { Some(s)=>s, None=>return false };
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => { if lengths.is_empty() {return false;} let r=bs.read(2)+3; let last=*lengths.last().unwrap(); for _ in 0..r {lengths.push(last);} },
            17 => { let r=bs.read(3)+3; for _ in 0..r {lengths.push(0);} },
            18 => { let r=bs.read(7)+11; for _ in 0..r {lengths.push(0);} },
            _ => return false,
        }
        if lengths.len() > n { return false; }
    }
    if !kraft_ok(&lengths[..hlit]) { return false; }
    if !kraft_ok(&lengths[hlit..hlit+hdist]) { return false; }
    true
}

fn main() {
    let data = fs::read("crack_input.bin").expect("crack_input.bin");
    let mut p = 0usize;
    let rd_u32 = |d:&[u8], p:&mut usize| { let v=u32::from_le_bytes([d[*p],d[*p+1],d[*p+2],d[*p+3]]); *p+=4; v };
    let plainlen = rd_u32(&data,&mut p) as usize;
    let d0 = data[p]; p+=1;
    let mut sets: Vec<Vec<u8>> = Vec::new();
    for _ in 0..5 {
        let n = u16::from_le_bytes([data[p],data[p+1]]) as usize; p+=2;
        sets.push(data[p..p+n].to_vec()); p+=n;
    }
    let taillen = rd_u32(&data,&mut p) as usize;
    let tail = data[p..p+taillen].to_vec();
    eprintln!("plainlen={} d0={:#x} setsizes={:?} taillen={}", plainlen, d0, sets.iter().map(|s|s.len()).collect::<Vec<_>>(), taillen);

    let tail = Arc::new(tail);
    let sets = Arc::new(sets);
    let found = Arc::new(AtomicBool::new(false));
    let result: Arc<Mutex<Option<[u8;5]>>> = Arc::new(Mutex::new(None));
    let nthreads = thread::available_parallelism().map(|n|n.get()).unwrap_or(8);
    let d1set = sets[0].clone();
    let chunk = (d1set.len() + nthreads - 1) / nthreads;
    eprintln!("threads={} d1chunk={}", nthreads, chunk);

    let mut handles = Vec::new();
    for t in 0..nthreads {
        let lo = t*chunk; if lo>=d1set.len() { break; }
        let hi = usize::min(lo+chunk, d1set.len());
        let d1slice: Vec<u8> = d1set[lo..hi].to_vec();
        let sets=Arc::clone(&sets); let tail=Arc::clone(&tail);
        let found=Arc::clone(&found); let result=Arc::clone(&result);
        handles.push(thread::spawn(move || {
            let s1=&sets[1]; let s2=&sets[2]; let s3=&sets[3]; let s4=&sets[4];
            let mut checked: u64 = 0;
            let mut inflates: u64 = 0;
            'outer: for &d1 in &d1slice {
                for &d2 in s1 {
                    for &d3 in s2 {
                        for &d4 in s3 {
                            for &d5 in s4 {
                                if found.load(Ordering::Relaxed) { break 'outer; }
                                checked+=1;
                                let guess=[d1,d2,d3,d4,d5];
                                if !header_ok(d0,&guess,&tail) { continue; }
                                inflates+=1;
                                // build zlib stream and inflate
                                let mut zs = Vec::with_capacity(2+1+5+tail.len());
                                zs.push(0x78); zs.push(0xda); zs.push(d0);
                                zs.extend_from_slice(&guess);
                                zs.extend_from_slice(&tail);
                                if let Ok(out) = miniz_oxide::inflate::decompress_to_vec_zlib(&zs) {
                                    if out.len()==plainlen {
                                        *result.lock().unwrap()=Some(guess);
                                        found.store(true,Ordering::Relaxed);
                                        eprintln!("HIT d[1..5]={:02x?} inflated {} bytes (checked {})", guess, out.len(), checked);
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            eprintln!("thread {} done checked={} inflates={}", t, checked, inflates);
        }));
    }
    for h in handles { let _=h.join(); }
    let res = *result.lock().unwrap();
    if let Some(g)=res {
        // emit the 5 recovered bytes as hex on stdout
        println!("{:02x}{:02x}{:02x}{:02x}{:02x}", g[0],g[1],g[2],g[3],g[4]);
    } else {
        eprintln!("NO HIT");
        std::process::exit(2);
    }
}
