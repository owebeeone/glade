// Independent named-semantic input consumer. Representation only, no auth.
import {readFileSync} from "node:fs";
import {createHash} from "node:crypto";
import {strict as assert} from "node:assert";
type Atom = null | boolean | bigint | string | Uint8Array | Atom[] | Map<number,Atom>;
type Json = null | boolean | string | number | Json[] | {[key:string]:Json};
interface Schema {kind:string;fields?:[string,string][];variants?:[string,[string,string][]][];}
const object=(v:Json):{[key:string]:Json}=>{if(v===null || typeof v!=="object" || Array.isArray(v)){throw Error("object");}return v;};
const text=(v:Json):string=>{if(typeof v!=="string"){throw Error("text");}return v;};
function integer(v:Json,signed=false):bigint {
 const source=text(v);const n=BigInt(source);
 if(String(n)!==source || n < (signed?-(1n<<63n):0n) || n>(signed?(1n<<63n)-1n:(1n<<64n)-1n)){throw Error("integer");}
 return n;
}
function head(major:number,n:bigint):Buffer {
 if(n<24n){return Buffer.from([(major<<5)|Number(n)]);}
 for(const [width,additional] of [[1,24],[2,25],[4,26],[8,27]]){
  if(n<(1n<<BigInt(width*8))){const out=Buffer.alloc(width+1);out[0]=(major<<5)|additional;for(let i=0;i<width;i++){out[width-i]=Number((n>>BigInt(i*8))&255n);}return out;}
 }
 throw Error("overflow");
}
function cbor(v:Atom):Buffer {
 if(v===null){return Buffer.from([246]);}
 if(typeof v==="boolean"){return Buffer.from([v?245:244]);}
 if(typeof v==="bigint"){return v>=0n?head(0,v):head(1,-1n-v);}
 if(typeof v==="string"){const b=Buffer.from(v,"utf8");return Buffer.concat([head(3,BigInt(b.length)),b]);}
 if(v instanceof Uint8Array){return Buffer.concat([head(2,BigInt(v.length)),v]);}
 if(Array.isArray(v)){return Buffer.concat([head(4,BigInt(v.length)),...v.map(cbor)]);}
 const fields=[...v.entries()].sort((a,b)=>a[0]-b[0]);return Buffer.concat([head(5,BigInt(fields.length)),...fields.flatMap(([k,value])=>[cbor(BigInt(k)),cbor(value)])]);
}
function bytes(v:Json):Buffer {const value=text(v);if(!/^(?:[0-9a-f]{2})*$/.test(value)){throw Error("hex");}return Buffer.from(value,"hex");}
function semantic(ty:string,v:Json,types:Record<string,Schema>):Atom {
 if(ty==="String"){return text(v);}
 if(ty==="Digest" || ty==="Bytes"){const out=bytes(v);if(ty==="Digest" && out.length!==32){throw Error("digest");}return out;}
 if(ty==="u64" || ty==="usize"){const out=Buffer.alloc(8);out.writeBigUInt64BE(integer(v));return out;}
 if(ty==="i64" || ty==="u8"){const n=integer(v,ty==="i64");if(ty==="u8" && n>255n){throw Error("byte");}return n;}
 if(ty==="bool"){if(typeof v!=="boolean"){throw Error("bool");}return v;}
 if(ty.startsWith("Vec<")){if(!Array.isArray(v)){throw Error("array");}return v.map(x=>semantic(ty.slice(4,-1),x,types));}
 if(ty.startsWith("Option<")){return v===null?null:[semantic(ty.slice(7,-1),v,types)];}
 if(ty.startsWith("Box<")){return semantic(ty.slice(4,-1),v,types);}
 if(ty==="Operation"){
  const o=object(v);if(!Array.isArray(o.refs)){throw Error("refs");}
  const refs=o.refs.map(h=>{const r=object(h);return new Map<number,Atom>([[1,text(r.origin)],[2,integer(r.seq,true)],[3,bytes(r.hash)]]);});
  return cbor(new Map<number,Atom>([[1,text(o.share)],[2,text(o.glade_id)],[3,bytes(o.key)],[4,text(o.origin)],[5,integer(o.seq,true)],[6,o.prev===null?null:bytes(o.prev)],[7,integer(o.lamport,true)],[8,refs],[9,integer(o.shape,true)],[10,bytes(o.payload)]]));
 }
 const schema=types[ty];const value=object(v);
 if(schema.kind==="enum"){
  const index=schema.variants!.findIndex(([name])=>name===value.variant);if(index<0){throw Error("enum");}
  const fields=schema.variants![index][1];const named=object(value.fields);assert.deepEqual(Object.keys(named).sort(),fields.map(([name])=>name).sort());return [BigInt(index),...fields.map(([name,t])=>semantic(t,named[name],types))];
 }
 const fields=schema.fields!;assert.deepEqual(Object.keys(value).sort(),fields.map(([name])=>name).sort());
 return new Map(fields.map(([name,t],index)=>[index,semantic(t,value[name],types)]));
}
function profile(kind:string,value:Json,types:Record<string,Schema>):Buffer {
 const body=semantic(kind,value,types);if(!(body instanceof Map)){throw Error("profile fields");}
 return cbor(new Map<number,Atom>([[0,1n],[1,kind],...[...body.entries()].map(([k,v]):[number,Atom]=>[k+2,v])]));
}
function decode(raw:Buffer,limits:{limit?:number;items?:number;depth?:number}={}):Atom {
 if(raw.length>(limits.limit??262144)){throw Error("limit");}let index=0;let remaining=limits.items??8192;
 function take(n:number):Buffer {if(n>raw.length-index){throw Error("truncated");}const out=raw.subarray(index,index+n);index+=n;return out;}
 function value(level:number):Atom {
  remaining--;if(remaining<0 || level>(limits.depth??64)){throw Error("budget");}const byte=take(1)[0];const major=byte>>5;const additional=byte&31;let n:bigint;
  if(additional<24){n=BigInt(additional);}else if(additional<=27){const width=2**(additional-24);n=0n;for(const b of take(width)){n=(n<<8n)|BigInt(b);}if(n<(width===1?24n:1n<<BigInt(width/2*8))){throw Error("nonshortest");}}else{throw Error("indefinite");}
  if(major===0 || major===1){if(n>(1n<<63n)-1n){throw Error("wire int");}return major===0?n:-1n-n;}
  if(major===2 || major===3){if(n>BigInt(raw.length-index)){throw Error("truncated");}if(major===3 && n>256n){throw Error("name");}const b=take(Number(n));return major===2?b:new TextDecoder("utf-8",{fatal:true}).decode(b);}
  if(major===4){if(n>BigInt(remaining)){throw Error("items");}return Array.from({length:Number(n)},()=>value(level+1));}
  if(major===5){if(n>BigInt(Math.floor(remaining/2))){throw Error("items");}const out=new Map<number,Atom>();let previous=-1n;for(let i=0;i<Number(n);i++){const key=value(level+1);if(typeof key!=="bigint" || key<=previous || key>BigInt(Number.MAX_SAFE_INTEGER)){throw Error("duplicate/order");}previous=key;out.set(Number(key),value(level+1));}return out;}
  if(major===7 && additional>=20 && additional<=22){return [false,true,null][additional-20];}throw Error("unsupported");
 }
 const out=value(0);if(index!==raw.length){throw Error("trailing");}return out;
}
function validateProfile(raw:Buffer,kind:string,types:Record<string,Schema>):Atom {
 const out=decode(raw);const count=types[kind].fields!.length+2;
 if(!(out instanceof Map) || out.size!==count || out.get(0)!==1n || out.get(1)!==kind || [...out.keys()].some((key,index)=>key!==index)){throw Error("closed profile");}
 if(kind==="SignedRecord"){
  const domains=["descriptor","origin-certificate","offline-permit","policy-cut","application-op","local-session","admission","inventory"].map(name=>`glade/ic3/v1/${name}\0`);
  const version=out.get(3);const signer=out.get(4);const sig=out.get(6);
  if(!domains.includes(out.get(2) as string) || !(version instanceof Uint8Array) || Buffer.from(version).readBigUInt64BE()!==1n || !(signer instanceof Uint8Array) || signer.length!==32 || !(sig instanceof Uint8Array) || sig.length!==64){throw Error("signature/domain shape");}
 }
 return out;
}
const root=process.argv[2];if(!root){throw Error("explicit vector root required");}
const input=JSON.parse(readFileSync(`${root}/vectors/input.json`,"utf8"));const types=JSON.parse(readFileSync(`${root}/vectors/schema.json`,"utf8")).types as Record<string,Schema>;
const expected=JSON.parse(readFileSync(`${root}/vectors/expected.json`,"utf8"));let count=0;
for(const vector of input.cases){const out=profile(vector.kind,vector.value,types);assert.equal(cbor(validateProfile(out,vector.kind,types)).toString("hex"),out.toString("hex"));assert.deepEqual({hex:out.toString("hex"),sha256:createHash("sha256").update(out).digest("hex")},expected[vector.id]);count++;}
const negatives=JSON.parse(readFileSync(`${root}/vectors/negative.json`,"utf8"));for(const vector of negatives.cases){assert.throws(()=>vector.kind?validateProfile(Buffer.from(vector.hex,"hex"),vector.kind,types):decode(Buffer.from(vector.hex,"hex"),vector.limits));}
console.log(`TypeScript independent profile vectors: ${count} positive + ${negatives.cases.length} negative controls PASS`);
