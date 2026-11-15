import { Bash } from "@sentropic/bash";
const b = new Bash({ cwd: 3 });
const n: number = b.getCwd();
void n;
