import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LuaVM, LuaError, LuaTable, luaToJS } from '../src/wm/lua-vm.js';

const run = (src) => new LuaVM().run(src);

test('arithmetic, precedence and string concatenation', () => {
  assert.equal(run('return 1 + 2 * 3 ^ 2'), 19);
  assert.equal(run('return 7 % 3'), 1);
  assert.equal(run('return "a" .. 1 .. "b"'), 'a1b');
  assert.equal(run('return #"hello"'), 5);
});

test('control flow and closures', () => {
  const src = `
    local function counter()
      local n = 0
      return function() n = n + 1; return n end
    end
    local c = counter()
    c(); c()
    local total = 0
    for i = 1, 10 do
      if i % 2 == 0 then total = total + i elseif i == 5 then break end
    end
    return c() * 100 + total`;
  assert.equal(run(src), 306); // c() == 3; 2 + 4 = 6 before break at 5
});

test('tables, ipairs and pairs', () => {
  const t = run('local t = {10, 20, x = "y", ["k"] = 4}; t[3] = 30; return t');
  assert.ok(t instanceof LuaTable);
  assert.equal(t.get(3), 30);
  assert.equal(t.get('x'), 'y');
  assert.equal(run('local s = 0; for _, v in ipairs({1, 2, 3}) do s = s + v end; return s'), 6);
  assert.equal(run('local n = 0; for k in pairs({a=1, b=2, c=3}) do n = n + 1 end; return n'), 3);
});

test('stdlib string and math helpers', () => {
  assert.equal(run('return string.upper("wm")'), 'WM');
  assert.equal(run('return math.max(3, 9, 4)'), 9);
  assert.equal(run('return tostring(12) .. type(nil)'), '12nil');
});

test('JS functions can be called from Lua', () => {
  const vm = new LuaVM();
  const calls = [];
  vm.setGlobal('record', (a, b) => { calls.push([a, b]); });
  vm.run('for i = 1, 3 do record(i, "ws" .. i) end');
  assert.deepEqual(calls, [[1, 'ws1'], [2, 'ws2'], [3, 'ws3']]);
});

test('luaToJS converts array-like and keyed tables', () => {
  assert.deepEqual(luaToJS(run('return {"a", "b"}')), ['a', 'b']);
  assert.deepEqual(luaToJS(run('return {width = 2, focused = "#fff"}')), { width: 2, focused: '#fff' });
});

test('runtime errors surface as LuaError with the chunk name', () => {
  assert.throws(() => new LuaVM().run('error("boom")', 'wm.lua'), (e) => e instanceof LuaError && /wm\.lua: boom/.test(e.message));
});

test('syntax errors are reported, not swallowed', () => {
  assert.throws(() => run('if true then'));
  assert.throws(() => run('local = 3'));
});
