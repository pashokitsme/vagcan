#!/usr/bin/env python3
"""Focused: RSA-decrypt (and public-op) the named handshake blobs + test oracle."""
import sys, hashlib
sys.path.insert(0, ".")
from Crypto.Cipher import AES
from usbpcap import reassemble_frames
from link_cipher import IV_TABLE
from extract_rsa_key import extract

IV = IV_TABLE[4]; WANT = bytes.fromhex("02a999f6da7c9c3a")
def oracle(k):
    if len(k) != 32: return None
    for m,fn in (("enc",lambda c:c.encrypt(IV)),("dec",lambda c:c.decrypt(IV))):
        ks=fn(AES.new(bytes(k),AES.MODE_ECB))
        if ks[6:14]==WANT: return (m,ks)
    return None

def test_bytes(label,buf):
    hits=[]
    for i in range(0,max(1,len(buf)-31)):
        w=buf[i:i+32]
        if len(w)==32:
            r=oracle(w)
            if r: hits.append((f"{label}[{i}]",w,r))
    for h in (hashlib.sha256(buf).digest(),hashlib.md5(buf).digest()*2):
        r=oracle(h)
        if r: hits.append((f"hash({label})",h,r))
    return hits

def main(path):
    key=extract(); n=key.n; d=key.d; e=key.e
    frames=list(reassemble_frames(path))
    named={}
    for f in frames:
        p=f["payload"]
        if not p: continue
        op=p[0]; named.setdefault((f["dir"],op),[]).append(bytes(p[1:]))
    print(f"# {path}")
    allhits=[]
    for (dr,op),lst in sorted(named.items()):
        cat=b"".join(lst)
        blobs={f"{dr}_{op:02x}_cat":cat}
        for i,pl in enumerate(lst[:4]): blobs[f"{dr}_{op:02x}_#{i}"]=pl
        for lbl,b in blobs.items():
            # direct oracle on raw bytes / hashes
            allhits+=test_bytes(f"raw:{lbl}",b)
            c=int.from_bytes(b,"big")
            if 0<c<n:
                for opn,ex in (("priv",d),("pub",e)):
                    m=pow(c,ex,n).to_bytes(128,"big")
                    allhits+=test_bytes(f"rsa_{opn}:{lbl}",m)
                    # pkcs unpad
                    if m[0]==0 and m[1] in (1,2):
                        z=m.find(b"\x00",2)
                        if z>0: allhits+=test_bytes(f"rsa_{opn}_pkcs:{lbl}",m[z+1:])
    # also b6||b7, 09out||09in cross combos
    b6=named.get(("OUT",0xb6),[b""])[0]; b7=named.get(("IN",0xb7),[b""])[0]
    o9=named.get(("OUT",0x09),[b""])[0]; i9=named.get(("IN",0x09),[b""])[0]
    i19=named.get(("IN",0x19),[b""])[0]
    for lbl,b in {"b6|b7":b6+b7,"b7|b6":b7+b6,"o9|i9":o9+i9,"b6|b7|i19":b6+b7+i19,
                  "b6[1:]":b6[1:],"i19":i19}.items():
        allhits+=test_bytes(f"comb:{lbl}",b)
        for k in (hashlib.sha256(b).digest(),):
            pass
    if allhits:
        for lab,k,r in allhits: print(f"*** HIT {lab} mode={r[0]} K={k.hex()}")
    else: print("# no hit on named handshake blobs")

if __name__=="__main__":
    main(sys.argv[1] if len(sys.argv)>1 else "../reading-ecus.pcapng")
