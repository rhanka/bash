// Tiny real core exercising canonical resource imports and indirect tuple ownership.
import { functions, layout, resourceKinds } from "../../host/core/abi/layout.js";
import { coreName, importsName, postName, watModule } from "./support.mjs";

const types = Object.fromEntries(functions.map((fn) => [fn.name, fn]));
const shell = types["[static]shell.create"];
const fiber = types["[static]fiber.spawn"];
export const shellTupleBytes = shell.params.size;
const trace = shell.params.fields[1].offset + shell.params.fields[1].type.fields.find(({ key }) => key === "trace").offset;
const ownerPayload = shell.result.payloadOffset;
const kernelError = layout("kernel-error");
const errorDetail = ownerPayload + kernelError.fields.find(({ key }) => key === "detail").offset;

export const reps = { vfs: 0x81234567, shell: 0x81234568, fiber: 0x81234569 };
const wrongKind = { vfs: "shell", shell: "fiber", fiber: "vfs" };

export function resourceModule(kind) {
  const imports = resourceKinds.map((name) => [
    '(import "' + importsName + '" "[resource-new]' + name + '" (func $new-' + name + ' (param i32) (result i32)))',
    '(import "' + importsName + '" "[resource-drop]' + name + '" (func $drop-' + name + ' (param i32)))',
  ].join("\n")).join("\n");
  const ownerFunctions = resourceKinds.map((name) => [
    '(global $host-' + name + ' (mut i32) (i32.const 0))',
    '(func $make-' + name + ' (result i32)',
    '  (global.set $host-' + name + ' (call $new-' + name + ' (i32.const ' + reps[name] + ')))',
    '  (i32.store8 (i32.const 128) (i32.const 0))',
    '  (i32.store offset=' + ownerPayload + ' (i32.const 128) (global.get $host-' + name + '))',
    '  (i32.const 128))',
    '(func $use-' + name + ' (param $rep i32)',
    '  (if (i32.ne (local.get $rep) (i32.const ' + reps[name] + ')) (then unreachable))',
    '  (if (global.get $dropBorrow) (then (call $drop-' + name + ' (global.get $host-' + name + ')))))',
    '(func (export "' + coreName("[dtor]" + name) + '") (param $rep i32)',
    '  (if (i32.ne (local.get $rep) (i32.const ' + reps[name] + ')) (then unreachable))',
    '  (if (global.get $trapDtor) (then unreachable)))',
  ].join("\n")).join("\n");
  const posts = ["configure", "[static]vfs.create", "[static]shell.create", "[static]fiber.spawn", "[method]shell.env",
    "[method]fiber.run", "[method]shell.register-command"].map((name) =>
    '(func (export "' + postName(name) + '") (param i32) (if (global.get $trapPost) (then unreachable)))').join("\n");
  return watModule(`(module
    ${imports}
    (memory (export "memory") 1)
    (global $next (mut i32) (i32.const 1024))
    (global $outer (export "outerPointer") (mut i32) (i32.const 0))
    (global $allocs (export "outerAllocs") (mut i32) (i32.const 0))
    (global $frees (export "outerFrees") (mut i32) (i32.const 0))
    (global $guest (mut i32) (i32.const 0))
    (global $dropBorrow (mut i32) (i32.const 0))
    (global $trapDtor (mut i32) (i32.const 0))
    (global $trapPost (mut i32) (i32.const 0))
    (global $trapAlloc (mut i32) (i32.const 0))
    (data (i32.const 512) "no")
    ${ownerFunctions}
    ${posts}
    (func $realloc (export "cabi_realloc") (param $old i32) (param $oldSize i32) (param $align i32) (param $size i32) (result i32)
      (local $ret i32)
      (if (global.get $trapAlloc) (then unreachable))
      (if (local.get $old)
        (then
          (if (i32.eq (local.get $oldSize) (i32.const ${shell.params.size}))
            (then
              (if (i32.ne (local.get $old) (global.get $outer)) (then unreachable))
              (global.set $frees (i32.add (global.get $frees) (i32.const 1)))))
          (return (i32.const 0))))
      (local.set $ret (i32.and
        (i32.add (global.get $next) (i32.sub (local.get $align) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get $align))))
      (global.set $next (i32.add (local.get $ret) (local.get $size)))
      (if (i32.eq (local.get $size) (i32.const ${shell.params.size}))
        (then
          (global.set $outer (local.get $ret))
          (global.set $allocs (i32.add (global.get $allocs) (i32.const 1)))))
      (local.get $ret))
    (func (export "${coreName("configure")}") (param $mode i64) (result i32)
      (if (i64.eq (local.get $mode) (i64.const 1)) (then
        (global.set $guest (call $new-${kind} (i32.const ${reps[kind]})))
        (call $drop-${kind} (global.get $guest))))
      (if (i64.eq (local.get $mode) (i64.const 2)) (then (call $drop-${kind} (global.get $guest))))
      (if (i64.eq (local.get $mode) (i64.const 3)) (then
        (call $drop-${wrongKind[kind]} (call $new-${kind} (i32.const ${reps[kind]})))))
      (if (i64.eq (local.get $mode) (i64.const 4)) (then (global.set $dropBorrow (i32.const 1))))
      (if (i64.eq (local.get $mode) (i64.const 5)) (then unreachable))
      (if (i64.eq (local.get $mode) (i64.const 6)) (then (global.set $trapDtor (i32.const 1))))
      (if (i64.eq (local.get $mode) (i64.const 7)) (then (global.set $trapPost (i32.const 1))))
      (if (i64.eq (local.get $mode) (i64.const 8)) (then (global.set $trapAlloc (i32.const 1))))
      (i32.store8 (i32.const 128) (i32.const 0))
      (i32.const 128))
    (func (export "${coreName("[static]vfs.create")}") (param i64) (result i32) (call $make-vfs))
    (func (export "${coreName("[static]shell.create")}") (param $ptr i32) (result i32)
      (local $reject i32)
      (call $use-vfs (i32.load (local.get $ptr)))
      (local.set $reject (i32.load8_u offset=${trace} (local.get $ptr)))
      (drop (call $realloc (local.get $ptr) (i32.const ${shell.params.size}) (i32.const ${shell.params.align}) (i32.const 0)))
      (if (result i32) (local.get $reject)
        (then
          (i32.store8 (i32.const 128) (i32.const 1))
          (i32.store8 offset=${ownerPayload} (i32.const 128) (i32.const ${layout("kernel-error-code").labels.indexOf("invalid-config")}))
          (i32.store offset=${errorDetail} (i32.const 128) (i32.const 512))
          (i32.store offset=${errorDetail + 4} (i32.const 128) (i32.const 2))
          (i32.const 128))
        (else (call $make-shell))))
    (func (export "${coreName("[static]fiber.spawn")}") (param $ptr i32) (result i32)
      (call $use-shell (i32.load (local.get $ptr)))
      (drop (call $realloc (local.get $ptr) (i32.const ${fiber.params.size}) (i32.const ${fiber.params.align}) (i32.const 0)))
      (call $make-fiber))
    (func (export "${coreName("[method]vfs.used-bytes")}") (param $rep i32) (result i64)
      (call $use-vfs (local.get $rep)) (i64.const -1))
    (func (export "${coreName("[method]shell.env")}") (param $rep i32) (result i32)
      (call $use-shell (local.get $rep)) (i32.const 256))
    (func (export "${coreName("[method]shell.register-command")}") (param $rep i32) (param i32 i32) (result i32)
      (call $use-shell (local.get $rep))
      (i32.store8 (i32.const 128) (i32.const 0)) (i32.const 128))
    (func (export "${coreName("[method]fiber.run")}") (param $rep i32) (param i64) (result i32)
      (call $use-fiber (local.get $rep))
      (i32.store8 (i32.const 256) (i32.const 1)) (i32.const 256))
  )`);
}

