import { Bash, InMemoryFs } from "@sentropic/bash";
const b = new Bash({ cwd: 3 });
const n: number = b.getCwd();
void n;
const c = new Bash({ fs: new InMemoryFs(), isolation: "sometimes" });
void c;
