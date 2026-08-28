/**
 * lua-vm.js — a compact, dependency-free Lua interpreter for the browser.
 *
 * This is the heart of Interstellar OS's "Lua-based" window manager: the shell
 * reads a real `.lua` config file and executes it here to drive the WM engine.
 *
 * It implements a practical subset of Lua 5.x — enough to express window-manager
 * configuration (layouts, gaps, keybindings, rules, workspaces) with real control
 * flow and data structures, without pulling in a WASM Lua runtime (which the
 * bundler-less static shell can't load anyway).
 *
 * Supported:
 *   - nil / booleans / numbers / strings (single, double, [[long]])
 *   - line comments (--) and block comments (--[[ ]])
 *   - tables: {1, 2, x = 3, ["k"] = 4}, indexing a.b / a["b"] / a[1]
 *   - local & global assignment (single and multiple: local a, b = 1, 2)
 *   - operators: + - * / % ^  ..  == ~= < > <= >=  and or not  # (unary - #)
 *   - if / elseif / else, while, numeric for, generic for (pairs/ipairs)
 *   - functions: function f() end, local function, anonymous, methods (obj:m)
 *   - multiple return values, return, break
 *   - stdlib: print, type, tostring, tonumber, pairs, ipairs, next, error,
 *             assert, select, string.*, math.*, table.*
 *
 * Intentionally omitted: coroutines, metatables, goto, bitwise ops, varargs
 * beyond basic `...`. The config surface never needs them.
 */

// ── nil sentinel ────────────────────────────────────────────────────────────
// We use JS `undefined` to mean Lua nil throughout.
const NIL = undefined;

// ── LuaError: carries a Lua-level error value ───────────────────────────────
export class LuaError extends Error {
  constructor(value) {
    super(typeof value === 'string' ? value : 'lua error');
    this.luaValue = value;
  }
}

// ── Break signal (internal control-flow marker) ─────────────────────────────
const BREAK = Symbol('break');

// ── LuaTable ────────────────────────────────────────────────────────────────
// Backed by a JS Map for arbitrary keys plus fast integer-array semantics.
export class LuaTable {
  constructor() {
    this.hash = new Map();
  }

  get(key) {
    if (key === NIL) return NIL;
    return this.hash.has(key) ? this.hash.get(key) : NIL;
  }

  set(key, value) {
    if (key === NIL) throw new LuaError('table index is nil');
    if (value === NIL) this.hash.delete(key);
    else this.hash.set(key, value);
  }

  // Lua `#t` — border of the array part.
  length() {
    let n = 0;
    while (this.hash.has(n + 1)) n++;
    return n;
  }

  insert(value) {
    this.set(this.length() + 1, value);
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. LEXER
// ═══════════════════════════════════════════════════════════════════════════

const KEYWORDS = new Set([
  'and', 'break', 'do', 'else', 'elseif', 'end', 'false', 'for', 'function',
  'if', 'in', 'local', 'nil', 'not', 'or', 'repeat', 'return', 'then',
  'true', 'until', 'while',
]);

function tokenize(src) {
  const tokens = [];
  let i = 0;
  let line = 1;
  const n = src.length;

  const isDigit = (c) => c >= '0' && c <= '9';
  const isAlpha = (c) => (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c === '_';
  const isAlphaNum = (c) => isAlpha(c) || isDigit(c);

  const push = (type, value) => tokens.push({ type, value, line });

  while (i < n) {
    const c = src[i];

    // Whitespace
    if (c === '\n') { line++; i++; continue; }
    if (c === ' ' || c === '\t' || c === '\r') { i++; continue; }

    // Comments
    if (c === '-' && src[i + 1] === '-') {
      i += 2;
      // Long comment --[[ ... ]]
      if (src[i] === '[' && src[i + 1] === '[') {
        i += 2;
        while (i < n && !(src[i] === ']' && src[i + 1] === ']')) {
          if (src[i] === '\n') line++;
          i++;
        }
        i += 2;
      } else {
        while (i < n && src[i] !== '\n') i++;
      }
      continue;
    }

    // Long string [[ ... ]]
    if (c === '[' && src[i + 1] === '[') {
      i += 2;
      let str = '';
      while (i < n && !(src[i] === ']' && src[i + 1] === ']')) {
        if (src[i] === '\n') line++;
        str += src[i++];
      }
      i += 2;
      push('string', str);
      continue;
    }

    // Strings
    if (c === '"' || c === "'") {
      const quote = c;
      i++;
      let str = '';
      while (i < n && src[i] !== quote) {
        if (src[i] === '\\') {
          i++;
          const esc = src[i++];
          if (esc === 'n') str += '\n';
          else if (esc === 't') str += '\t';
          else if (esc === 'r') str += '\r';
          else if (esc === '\\') str += '\\';
          else if (esc === '"') str += '"';
          else if (esc === "'") str += "'";
          else if (esc === '0') str += '\0';
          else str += esc;
        } else {
          if (src[i] === '\n') line++;
          str += src[i++];
        }
      }
      i++; // closing quote
      push('string', str);
      continue;
    }

    // Numbers (int, float, hex)
    if (isDigit(c) || (c === '.' && isDigit(src[i + 1]))) {
      let numStr = '';
      if (c === '0' && (src[i + 1] === 'x' || src[i + 1] === 'X')) {
        numStr = '0x';
        i += 2;
        while (i < n && /[0-9a-fA-F]/.test(src[i])) numStr += src[i++];
        push('number', parseInt(numStr, 16));
        continue;
      }
      while (i < n && (isDigit(src[i]) || src[i] === '.')) numStr += src[i++];
      // Exponent
      if (src[i] === 'e' || src[i] === 'E') {
        numStr += src[i++];
        if (src[i] === '+' || src[i] === '-') numStr += src[i++];
        while (i < n && isDigit(src[i])) numStr += src[i++];
      }
      push('number', parseFloat(numStr));
      continue;
    }

    // Identifiers / keywords
    if (isAlpha(c)) {
      let id = '';
      while (i < n && isAlphaNum(src[i])) id += src[i++];
      if (KEYWORDS.has(id)) push(id, id);
      else push('name', id);
      continue;
    }

    // Operators & punctuation (longest match first)
    const three = src.substr(i, 3);
    if (three === '...') { push('...', '...'); i += 3; continue; }

    const two = src.substr(i, 2);
    if (['==', '~=', '<=', '>=', '..', '::'].includes(two)) {
      push(two, two); i += 2; continue;
    }

    if ('+-*/%^#<>=(){}[];:,.'.includes(c)) {
      push(c, c); i++; continue;
    }

    throw new LuaError(`unexpected character '${c}' at line ${line}`);
  }

  push('eof', null);
  return tokens;
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. PARSER (recursive descent → AST)
// ═══════════════════════════════════════════════════════════════════════════

function parse(tokens) {
  let pos = 0;

  const peek = (o = 0) => tokens[pos + o];
  const next = () => tokens[pos++];
  const check = (type) => peek().type === type;
  const accept = (type) => (check(type) ? next() : null);
  const expect = (type) => {
    if (!check(type)) {
      const t = peek();
      throw new LuaError(`expected '${type}' but got '${t.type}' at line ${t.line}`);
    }
    return next();
  };

  // block := {statement} [returnstat]
  function parseBlock(terminators) {
    const stmts = [];
    while (!check('eof') && !terminators.includes(peek().type)) {
      if (check('return')) {
        stmts.push(parseReturn());
        break;
      }
      const s = parseStatement();
      if (s) stmts.push(s);
    }
    return { type: 'Block', body: stmts };
  }

  function parseReturn() {
    const line = expect('return').line;
    const args = [];
    const stops = ['end', 'else', 'elseif', 'until', 'eof'];
    if (!stops.includes(peek().type) && !check(';')) {
      args.push(parseExpr());
      while (accept(',')) args.push(parseExpr());
    }
    accept(';');
    return { type: 'Return', args, line };
  }

  function parseStatement() {
    const t = peek();
    switch (t.type) {
      case ';': next(); return null;
      case 'local': return parseLocal();
      case 'if': return parseIf();
      case 'while': return parseWhile();
      case 'for': return parseFor();
      case 'function': return parseFunctionStmt();
      case 'do': {
        next();
        const body = parseBlock(['end']);
        expect('end');
        return { type: 'Do', body };
      }
      case 'break': next(); return { type: 'Break', line: t.line };
      case 'repeat': return parseRepeat();
      default: return parseExprStatement();
    }
  }

  function parseLocal() {
    const line = expect('local').line;
    if (accept('function')) {
      const name = expect('name').value;
      const fn = parseFunctionBody(false);
      return { type: 'LocalFunction', name, fn, line };
    }
    const names = [expect('name').value];
    while (accept(',')) names.push(expect('name').value);
    let exprs = [];
    if (accept('=')) {
      exprs.push(parseExpr());
      while (accept(',')) exprs.push(parseExpr());
    }
    return { type: 'Local', names, exprs, line };
  }

  function parseIf() {
    const line = expect('if').line;
    const clauses = [];
    const cond = parseExpr();
    expect('then');
    clauses.push({ cond, body: parseBlock(['end', 'else', 'elseif']) });
    while (check('elseif')) {
      next();
      const c = parseExpr();
      expect('then');
      clauses.push({ cond: c, body: parseBlock(['end', 'else', 'elseif']) });
    }
    let elseBody = null;
    if (accept('else')) elseBody = parseBlock(['end']);
    expect('end');
    return { type: 'If', clauses, elseBody, line };
  }

  function parseWhile() {
    const line = expect('while').line;
    const cond = parseExpr();
    expect('do');
    const body = parseBlock(['end']);
    expect('end');
    return { type: 'While', cond, body, line };
  }

  function parseRepeat() {
    const line = expect('repeat').line;
    const body = parseBlock(['until']);
    expect('until');
    const cond = parseExpr();
    return { type: 'Repeat', body, cond, line };
  }

  function parseFor() {
    const line = expect('for').line;
    const first = expect('name').value;
    if (check('=')) {
      next();
      const start = parseExpr();
      expect(',');
      const limit = parseExpr();
      let step = null;
      if (accept(',')) step = parseExpr();
      expect('do');
      const body = parseBlock(['end']);
      expect('end');
      return { type: 'NumericFor', var: first, start, limit, step, body, line };
    }
    // Generic for
    const names = [first];
    while (accept(',')) names.push(expect('name').value);
    expect('in');
    const exprs = [parseExpr()];
    while (accept(',')) exprs.push(parseExpr());
    expect('do');
    const body = parseBlock(['end']);
    expect('end');
    return { type: 'GenericFor', names, exprs, body, line };
  }

  function parseFunctionStmt() {
    const line = expect('function').line;
    // funcname := name {'.' name} [':' name]
    let target = { type: 'Name', name: expect('name').value };
    let isMethod = false;
    while (check('.')) {
      next();
      const key = expect('name').value;
      target = { type: 'Index', obj: target, key: { type: 'String', value: key } };
    }
    if (accept(':')) {
      const key = expect('name').value;
      target = { type: 'Index', obj: target, key: { type: 'String', value: key } };
      isMethod = true;
    }
    const fn = parseFunctionBody(isMethod);
    return { type: 'Assign', targets: [target], exprs: [fn], line };
  }

  function parseFunctionBody(isMethod) {
    expect('(');
    const params = [];
    let hasVararg = false;
    if (isMethod) params.push('self');
    if (!check(')')) {
      do {
        if (check('...')) { next(); hasVararg = true; break; }
        params.push(expect('name').value);
      } while (accept(','));
    }
    expect(')');
    const body = parseBlock(['end']);
    expect('end');
    return { type: 'Function', params, hasVararg, body };
  }

  // Expression statement: assignment or function call
  function parseExprStatement() {
    const line = peek().line;
    const expr = parseSuffixed();
    if (check('=') || check(',')) {
      const targets = [expr];
      while (accept(',')) targets.push(parseSuffixed());
      expect('=');
      const exprs = [parseExpr()];
      while (accept(',')) exprs.push(parseExpr());
      return { type: 'Assign', targets, exprs, line };
    }
    if (expr.type !== 'Call' && expr.type !== 'MethodCall') {
      throw new LuaError(`syntax error: unexpected expression statement at line ${line}`);
    }
    return { type: 'ExprStatement', expr, line };
  }

  // ── Expressions with precedence climbing ──
  const BINARY_PREC = {
    'or': 1, 'and': 2,
    '<': 3, '>': 3, '<=': 3, '>=': 3, '~=': 3, '==': 3,
    '..': 5,
    '+': 6, '-': 6,
    '*': 7, '/': 7, '%': 7,
    '^': 10,
  };
  const RIGHT_ASSOC = new Set(['..', '^']);
  const UNARY_PREC = 8;

  function parseExpr(minPrec = 0) {
    let left = parseUnary();
    while (true) {
      const op = peek().type;
      const prec = BINARY_PREC[op];
      if (prec === undefined || prec < minPrec) break;
      next();
      const nextMin = RIGHT_ASSOC.has(op) ? prec : prec + 1;
      const right = parseExpr(nextMin);
      left = { type: 'Binary', op, left, right };
    }
    return left;
  }

  function parseUnary() {
    const t = peek();
    if (t.type === 'not' || t.type === '-' || t.type === '#') {
      next();
      const operand = parseExpr(UNARY_PREC);
      return { type: 'Unary', op: t.type, operand };
    }
    return parsePow();
  }

  // ^ binds tighter than unary on its left operand
  function parsePow() {
    let base = parseSuffixed();
    if (check('^')) {
      next();
      const exp = parseExpr(UNARY_PREC); // right side may include unary
      return { type: 'Binary', op: '^', left: base, right: exp };
    }
    return base;
  }

  // primary with suffixes: calls, indexing, method calls
  function parseSuffixed() {
    let expr = parsePrimary();
    while (true) {
      const t = peek();
      if (t.type === '.') {
        next();
        const key = expect('name').value;
        expr = { type: 'Index', obj: expr, key: { type: 'String', value: key } };
      } else if (t.type === '[') {
        next();
        const key = parseExpr();
        expect(']');
        expr = { type: 'Index', obj: expr, key };
      } else if (t.type === '(') {
        const args = parseArgs();
        expr = { type: 'Call', fn: expr, args };
      } else if (t.type === 'string') {
        // f "str" sugar
        next();
        expr = { type: 'Call', fn: expr, args: [{ type: 'String', value: t.value }] };
      } else if (t.type === '{') {
        // f {table} sugar
        const tbl = parseTable();
        expr = { type: 'Call', fn: expr, args: [tbl] };
      } else if (t.type === ':') {
        next();
        const method = expect('name').value;
        let args;
        if (check('string')) {
          const s = next();
          args = [{ type: 'String', value: s.value }];
        } else if (check('{')) {
          args = [parseTable()];
        } else {
          args = parseArgs();
        }
        expr = { type: 'MethodCall', obj: expr, method, args };
      } else {
        break;
      }
    }
    return expr;
  }

  function parseArgs() {
    expect('(');
    const args = [];
    if (!check(')')) {
      args.push(parseExpr());
      while (accept(',')) args.push(parseExpr());
    }
    expect(')');
    return args;
  }

  function parsePrimary() {
    const t = peek();
    switch (t.type) {
      case 'number': next(); return { type: 'Number', value: t.value };
      case 'string': next(); return { type: 'String', value: t.value };
      case 'nil': next(); return { type: 'Nil' };
      case 'true': next(); return { type: 'Boolean', value: true };
      case 'false': next(); return { type: 'Boolean', value: false };
      case '...': next(); return { type: 'Vararg' };
      case 'name': next(); return { type: 'Name', name: t.value };
      case 'function': next(); return parseFunctionBody(false);
      case '(': {
        next();
        const e = parseExpr();
        expect(')');
        return { type: 'Paren', expr: e };
      }
      case '{': return parseTable();
      default:
        throw new LuaError(`unexpected token '${t.type}' at line ${t.line}`);
    }
  }

  function parseTable() {
    expect('{');
    const fields = [];
    while (!check('}')) {
      if (check('[')) {
        next();
        const key = parseExpr();
        expect(']');
        expect('=');
        const value = parseExpr();
        fields.push({ kind: 'keyed', key, value });
      } else if (check('name') && peek(1).type === '=') {
        const key = next().value;
        next(); // =
        const value = parseExpr();
        fields.push({ kind: 'named', key, value });
      } else {
        fields.push({ kind: 'array', value: parseExpr() });
      }
      if (!accept(',') && !accept(';')) break;
    }
    expect('}');
    return { type: 'Table', fields };
  }

  const chunk = parseBlock(['eof']);
  expect('eof');
  return chunk;
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. INTERPRETER (tree-walking)
// ═══════════════════════════════════════════════════════════════════════════

// Scope: chained variable environments for locals.
class Scope {
  constructor(parent = null) {
    this.vars = new Map();
    this.parent = parent;
  }
  get(name) {
    let s = this;
    while (s) {
      if (s.vars.has(name)) return s.vars.get(name);
      s = s.parent;
    }
    return undefined;
  }
  has(name) {
    let s = this;
    while (s) {
      if (s.vars.has(name)) return true;
      s = s.parent;
    }
    return false;
  }
  setExisting(name, value) {
    let s = this;
    while (s) {
      if (s.vars.has(name)) { s.vars.set(name, value); return true; }
      s = s.parent;
    }
    return false;
  }
  declare(name, value) {
    this.vars.set(name, value);
  }
}

// A Return unwinds via a thrown control object.
class ReturnSignal {
  constructor(values) { this.values = values; }
}

export class LuaVM {
  constructor() {
    this.globals = new LuaTable();
    this._output = [];
    this.installStdlib();
  }

  // Redirect print() output to a callback (used by the Terminal app).
  setPrintHandler(fn) { this._printHandler = fn; }

  // Expose a JS value into the Lua global environment.
  setGlobal(name, value) { this.globals.set(name, value); }
  getGlobal(name) { return this.globals.get(name); }

  run(source, chunkName = 'config') {
    const tokens = tokenize(source);
    const ast = parse(tokens);
    const scope = new Scope();
    try {
      this.execBlock(ast, scope);
    } catch (e) {
      if (e instanceof ReturnSignal) return e.values[0];
      if (e instanceof LuaError) {
        throw new LuaError(`${chunkName}: ${luaToString(e.luaValue)}`);
      }
      throw e;
    }
  }

  // ── Statement execution ──
  execBlock(block, scope) {
    for (const stmt of block.body) {
      const r = this.execStatement(stmt, scope);
      if (r === BREAK) return BREAK;
    }
  }

  execStatement(stmt, scope) {
    switch (stmt.type) {
      case 'Local': {
        const values = this.evalExprList(stmt.exprs, scope, stmt.names.length);
        stmt.names.forEach((name, idx) => scope.declare(name, values[idx]));
        return;
      }
      case 'LocalFunction': {
        // Declare first so the function can recurse.
        scope.declare(stmt.name, undefined);
        const fn = this.makeFunction(stmt.fn, scope);
        scope.setExisting(stmt.name, fn);
        return;
      }
      case 'Assign': {
        const values = this.evalExprList(stmt.exprs, scope, stmt.targets.length);
        stmt.targets.forEach((target, idx) => this.assign(target, values[idx], scope));
        return;
      }
      case 'ExprStatement':
        this.evalExpr(stmt.expr, scope);
        return;
      case 'Do':
        return this.execBlock(stmt.body, new Scope(scope));
      case 'If': {
        for (const clause of stmt.clauses) {
          if (truthy(this.evalExpr(clause.cond, scope))) {
            return this.execBlock(clause.body, new Scope(scope));
          }
        }
        if (stmt.elseBody) return this.execBlock(stmt.elseBody, new Scope(scope));
        return;
      }
      case 'While': {
        while (truthy(this.evalExpr(stmt.cond, scope))) {
          const r = this.execBlock(stmt.body, new Scope(scope));
          if (r === BREAK) break;
        }
        return;
      }
      case 'Repeat': {
        do {
          const inner = new Scope(scope);
          const r = this.execBlock(stmt.body, inner);
          if (r === BREAK) break;
          if (truthy(this.evalExpr(stmt.cond, inner))) break;
        } while (true);
        return;
      }
      case 'NumericFor': {
        let val = tonumber(this.evalExpr(stmt.start, scope));
        const limit = tonumber(this.evalExpr(stmt.limit, scope));
        const step = stmt.step ? tonumber(this.evalExpr(stmt.step, scope)) : 1;
        if (val === undefined || limit === undefined || step === undefined) {
          throw new LuaError("'for' initial value must be a number");
        }
        while ((step > 0 && val <= limit) || (step < 0 && val >= limit)) {
          const inner = new Scope(scope);
          inner.declare(stmt.var, val);
          const r = this.execBlock(stmt.body, inner);
          if (r === BREAK) break;
          val += step;
        }
        return;
      }
      case 'GenericFor': {
        const state = this.evalExprList(stmt.exprs, scope, 3);
        const iterFn = state[0];
        const iterState = state[1];
        let control = state[2];
        while (true) {
          const results = this.callFunction(iterFn, [iterState, control]);
          const first = results[0];
          if (first === undefined) break;
          control = first;
          const inner = new Scope(scope);
          stmt.names.forEach((name, idx) => inner.declare(name, results[idx]));
          const r = this.execBlock(stmt.body, inner);
          if (r === BREAK) break;
        }
        return;
      }
      case 'Return': {
        const values = this.evalExprList(stmt.args, scope, -1);
        throw new ReturnSignal(values);
      }
      case 'Break':
        return BREAK;
      default:
        throw new LuaError(`cannot execute statement '${stmt.type}'`);
    }
  }

  assign(target, value, scope) {
    if (target.type === 'Name') {
      if (!scope.setExisting(target.name, value)) {
        this.globals.set(target.name, value);
      }
    } else if (target.type === 'Index') {
      const obj = this.evalExpr(target.obj, scope);
      const key = this.evalExpr(target.key, scope);
      if (obj instanceof LuaTable) {
        obj.set(key, value);
      } else {
        throw new LuaError(`attempt to index a ${luaType(obj)} value`);
      }
    } else {
      throw new LuaError('cannot assign to this expression');
    }
  }

  // Evaluate an expression list, expanding the final multi-value call.
  // want = -1 means "all"; otherwise pad/truncate to `want`.
  evalExprList(exprs, scope, want) {
    const values = [];
    exprs.forEach((expr, idx) => {
      const isLast = idx === exprs.length - 1;
      if (isLast && (expr.type === 'Call' || expr.type === 'MethodCall' || expr.type === 'Vararg')) {
        const multi = this.evalMulti(expr, scope);
        for (const v of multi) values.push(v);
      } else {
        values.push(this.evalExpr(expr, scope));
      }
    });
    if (want === -1) return values;
    while (values.length < want) values.push(undefined);
    return values.slice(0, want);
  }

  // Evaluate to (possibly) multiple values.
  evalMulti(expr, scope) {
    if (expr.type === 'Call') {
      const fn = this.evalExpr(expr.fn, scope);
      const args = this.evalExprList(expr.args, scope, -1);
      return this.callFunction(fn, args);
    }
    if (expr.type === 'MethodCall') {
      const obj = this.evalExpr(expr.obj, scope);
      const method = obj instanceof LuaTable ? obj.get(expr.method) : undefined;
      const args = this.evalExprList(expr.args, scope, -1);
      return this.callFunction(method, [obj, ...args]);
    }
    if (expr.type === 'Vararg') {
      return scope.get('...') || [];
    }
    return [this.evalExpr(expr, scope)];
  }

  // ── Expression evaluation (single value) ──
  evalExpr(expr, scope) {
    switch (expr.type) {
      case 'Number': return expr.value;
      case 'String': return expr.value;
      case 'Boolean': return expr.value;
      case 'Nil': return undefined;
      case 'Vararg': return (scope.get('...') || [])[0];
      case 'Name': {
        if (scope.has(expr.name)) return scope.get(expr.name);
        return this.globals.get(expr.name);
      }
      case 'Paren': return this.evalExpr(expr.expr, scope);
      case 'Index': {
        const obj = this.evalExpr(expr.obj, scope);
        const key = this.evalExpr(expr.key, scope);
        return this.index(obj, key);
      }
      case 'Call':
      case 'MethodCall':
        return this.evalMulti(expr, scope)[0];
      case 'Function':
        return this.makeFunction(expr, scope);
      case 'Table': {
        const tbl = new LuaTable();
        let arrayIdx = 1;
        expr.fields.forEach((field, i) => {
          if (field.kind === 'array') {
            const isLast = i === expr.fields.length - 1;
            if (isLast && (field.value.type === 'Call' || field.value.type === 'MethodCall')) {
              const multi = this.evalMulti(field.value, scope);
              for (const v of multi) tbl.set(arrayIdx++, v);
            } else {
              tbl.set(arrayIdx++, this.evalExpr(field.value, scope));
            }
          } else if (field.kind === 'named') {
            tbl.set(field.key, this.evalExpr(field.value, scope));
          } else {
            const k = this.evalExpr(field.key, scope);
            tbl.set(k, this.evalExpr(field.value, scope));
          }
        });
        return tbl;
      }
      case 'Binary': return this.evalBinary(expr, scope);
      case 'Unary': return this.evalUnary(expr, scope);
      default:
        throw new LuaError(`cannot evaluate expression '${expr.type}'`);
    }
  }

  index(obj, key) {
    if (obj instanceof LuaTable) return obj.get(key);
    if (typeof obj === 'string') {
      // string methods via the string library
      const strlib = this.globals.get('string');
      if (strlib instanceof LuaTable) return strlib.get(key);
    }
    throw new LuaError(`attempt to index a ${luaType(obj)} value`);
  }

  evalBinary(expr, scope) {
    const op = expr.op;
    // Short-circuit logical operators
    if (op === 'and') {
      const l = this.evalExpr(expr.left, scope);
      return truthy(l) ? this.evalExpr(expr.right, scope) : l;
    }
    if (op === 'or') {
      const l = this.evalExpr(expr.left, scope);
      return truthy(l) ? l : this.evalExpr(expr.right, scope);
    }

    const l = this.evalExpr(expr.left, scope);
    const r = this.evalExpr(expr.right, scope);

    switch (op) {
      case '+': return arith(l, r, (a, b) => a + b, '+');
      case '-': return arith(l, r, (a, b) => a - b, '-');
      case '*': return arith(l, r, (a, b) => a * b, '*');
      case '/': return arith(l, r, (a, b) => a / b, '/');
      case '%': return arith(l, r, (a, b) => a - Math.floor(a / b) * b, '%');
      case '^': return arith(l, r, (a, b) => Math.pow(a, b), '^');
      case '..': {
        const concatOk = (x) => typeof x === 'string' || typeof x === 'number';
        if (concatOk(l) && concatOk(r)) return luaToString(l) + luaToString(r);
        throw new LuaError(`attempt to concatenate a ${luaType(concatOk(l) ? r : l)} value`);
      }
      case '==': return luaEquals(l, r);
      case '~=': return !luaEquals(l, r);
      case '<': return compare(l, r) < 0;
      case '>': return compare(l, r) > 0;
      case '<=': return compare(l, r) <= 0;
      case '>=': return compare(l, r) >= 0;
      default:
        throw new LuaError(`unknown operator '${op}'`);
    }
  }

  evalUnary(expr, scope) {
    const v = this.evalExpr(expr.operand, scope);
    switch (expr.op) {
      case '-': {
        const n = tonumber(v);
        if (n === undefined) throw new LuaError(`attempt to perform arithmetic on a ${luaType(v)} value`);
        return -n;
      }
      case 'not': return !truthy(v);
      case '#':
        if (typeof v === 'string') return v.length;
        if (v instanceof LuaTable) return v.length();
        throw new LuaError(`attempt to get length of a ${luaType(v)} value`);
      default:
        throw new LuaError(`unknown unary operator '${expr.op}'`);
    }
  }

  // Build a Lua closure from a Function AST node.
  makeFunction(node, defScope) {
    const vm = this;
    const fn = function (...args) {
      const local = new Scope(defScope);
      node.params.forEach((p, i) => local.declare(p, args[i]));
      if (node.hasVararg) {
        local.declare('...', args.slice(node.params.length));
      }
      try {
        vm.execBlock(node.body, local);
      } catch (e) {
        if (e instanceof ReturnSignal) return e.values;
        throw e;
      }
      return [];
    };
    fn.__lua = true;
    return fn;
  }

  // Call either a Lua closure (returns array) or a native JS function.
  callFunction(fn, args) {
    if (typeof fn !== 'function') {
      throw new LuaError(`attempt to call a ${luaType(fn)} value`);
    }
    const result = fn.apply(this, args);
    if (Array.isArray(result)) return result;
    if (result === undefined) return [];
    return [result];
  }

  // ── Standard library ──
  installStdlib() {
    const g = this.globals;
    const vm = this;

    g.set('print', (...args) => {
      const line = args.map(luaToString).join('\t');
      if (vm._printHandler) vm._printHandler(line);
      else console.log(line);
      vm._output.push(line);
    });

    g.set('type', (v) => luaType(v));
    g.set('tostring', (v) => luaToString(v));
    g.set('tonumber', (v, base) => {
      if (base !== undefined && typeof v === 'string') {
        const n = parseInt(v, base);
        return Number.isNaN(n) ? undefined : n;
      }
      return tonumber(v);
    });

    g.set('error', (msg) => { throw new LuaError(msg); });
    g.set('assert', (v, msg) => {
      if (!truthy(v)) throw new LuaError(msg !== undefined ? msg : 'assertion failed!');
      return v;
    });

    g.set('select', (n, ...rest) => {
      if (n === '#') return rest.length;
      return rest.slice(tonumber(n) - 1);
    });

    g.set('rawget', (t, k) => (t instanceof LuaTable ? t.get(k) : undefined));
    g.set('rawset', (t, k, v) => { if (t instanceof LuaTable) t.set(k, v); return t; });

    // next / pairs / ipairs
    const next = (t, key) => {
      if (!(t instanceof LuaTable)) throw new LuaError("bad argument to 'next'");
      const keys = [...t.hash.keys()];
      if (key === undefined) {
        if (keys.length === 0) return [undefined];
        return [keys[0], t.hash.get(keys[0])];
      }
      const idx = keys.indexOf(key);
      if (idx === -1 || idx === keys.length - 1) return [undefined];
      const nk = keys[idx + 1];
      return [nk, t.hash.get(nk)];
    };
    g.set('next', next);
    g.set('pairs', (t) => [next, t, undefined]);
    g.set('ipairs', (t) => {
      const iter = (tbl, i) => {
        const ni = i + 1;
        const v = tbl.get(ni);
        if (v === undefined) return [undefined];
        return [ni, v];
      };
      return [iter, t, 0];
    });

    g.set('unpack', (t) => {
      const out = [];
      const len = t.length();
      for (let i = 1; i <= len; i++) out.push(t.get(i));
      return out;
    });

    // ── string library ──
    const stringLib = new LuaTable();
    stringLib.set('format', (fmt, ...args) => luaFormat(fmt, args));
    stringLib.set('len', (s) => String(s).length);
    stringLib.set('sub', (s, i, j) => {
      s = String(s);
      const len = s.length;
      i = i === undefined ? 1 : tonumber(i);
      j = j === undefined ? -1 : tonumber(j);
      if (i < 0) i = Math.max(len + i + 1, 1);
      else if (i === 0) i = 1;
      if (j < 0) j = len + j + 1;
      else if (j > len) j = len;
      if (i > j) return '';
      return s.substring(i - 1, j);
    });
    stringLib.set('upper', (s) => String(s).toUpperCase());
    stringLib.set('lower', (s) => String(s).toLowerCase());
    stringLib.set('rep', (s, n) => String(s).repeat(Math.max(0, tonumber(n))));
    stringLib.set('reverse', (s) => String(s).split('').reverse().join(''));
    stringLib.set('byte', (s, i) => String(s).charCodeAt((i || 1) - 1));
    stringLib.set('char', (...codes) => codes.map((c) => String.fromCharCode(c)).join(''));
    stringLib.set('find', (s, pat, init) => {
      s = String(s);
      const start = init ? tonumber(init) - 1 : 0;
      const idx = s.indexOf(pat, start); // plain substring find (no patterns)
      if (idx === -1) return [undefined];
      return [idx + 1, idx + pat.length];
    });
    stringLib.set('gsub', (s, pat, repl) => {
      s = String(s);
      const out = s.split(pat).join(repl);
      return [out, (s.split(pat).length - 1)];
    });
    g.set('string', stringLib);

    // ── math library ──
    const mathLib = new LuaTable();
    mathLib.set('pi', Math.PI);
    mathLib.set('huge', Infinity);
    mathLib.set('floor', (x) => Math.floor(tonumber(x)));
    mathLib.set('ceil', (x) => Math.ceil(tonumber(x)));
    mathLib.set('abs', (x) => Math.abs(tonumber(x)));
    mathLib.set('max', (...a) => Math.max(...a.map(tonumber)));
    mathLib.set('min', (...a) => Math.min(...a.map(tonumber)));
    mathLib.set('sqrt', (x) => Math.sqrt(tonumber(x)));
    mathLib.set('sin', (x) => Math.sin(tonumber(x)));
    mathLib.set('cos', (x) => Math.cos(tonumber(x)));
    mathLib.set('random', (m, n) => {
      if (m === undefined) return Math.random();
      if (n === undefined) return Math.floor(Math.random() * m) + 1;
      return Math.floor(Math.random() * (n - m + 1)) + m;
    });
    mathLib.set('randomseed', () => undefined);
    mathLib.set('pow', (x, y) => Math.pow(tonumber(x), tonumber(y)));
    g.set('math', mathLib);

    // ── table library ──
    const tableLib = new LuaTable();
    tableLib.set('insert', (t, a, b) => {
      if (b === undefined) t.insert(a);
      else {
        const pos = tonumber(a);
        const len = t.length();
        for (let i = len; i >= pos; i--) t.set(i + 1, t.get(i));
        t.set(pos, b);
      }
    });
    tableLib.set('remove', (t, pos) => {
      const len = t.length();
      if (len === 0) return undefined;
      pos = pos === undefined ? len : tonumber(pos);
      const removed = t.get(pos);
      for (let i = pos; i < len; i++) t.set(i, t.get(i + 1));
      t.set(len, undefined);
      return removed;
    });
    tableLib.set('concat', (t, sep) => {
      sep = sep === undefined ? '' : String(sep);
      const parts = [];
      const len = t.length();
      for (let i = 1; i <= len; i++) parts.push(luaToString(t.get(i)));
      return parts.join(sep);
    });
    tableLib.set('getn', (t) => t.length());
    tableLib.set('sort', (t, comp) => {
      const len = t.length();
      const arr = [];
      for (let i = 1; i <= len; i++) arr.push(t.get(i));
      arr.sort((a, b) => {
        if (comp) return truthy(vm.callFunction(comp, [a, b])[0]) ? -1 : 1;
        return compare(a, b);
      });
      arr.forEach((v, i) => t.set(i + 1, v));
    });
    g.set('table', tableLib);
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. RUNTIME HELPERS
// ═══════════════════════════════════════════════════════════════════════════

function truthy(v) {
  return v !== undefined && v !== false && v !== null;
}

function luaType(v) {
  if (v === undefined || v === null) return 'nil';
  if (typeof v === 'boolean') return 'boolean';
  if (typeof v === 'number') return 'number';
  if (typeof v === 'string') return 'string';
  if (v instanceof LuaTable) return 'table';
  if (typeof v === 'function') return 'function';
  return 'userdata';
}

function luaToString(v) {
  if (v === undefined || v === null) return 'nil';
  if (typeof v === 'boolean') return v ? 'true' : 'false';
  if (typeof v === 'number') {
    if (Number.isInteger(v)) return String(v);
    return String(v);
  }
  if (typeof v === 'string') return v;
  if (v instanceof LuaTable) return 'table: 0x' + (v.__id || (v.__id = (Math.random() * 0xffffff | 0).toString(16)));
  if (typeof v === 'function') return 'function: builtin';
  return String(v);
}

function tonumber(v) {
  if (typeof v === 'number') return v;
  if (typeof v === 'string') {
    const t = v.trim();
    if (t === '') return undefined;
    const n = Number(t);
    return Number.isNaN(n) ? undefined : n;
  }
  return undefined;
}

function arith(l, r, fn, op) {
  const a = tonumber(l);
  const b = tonumber(r);
  if (a === undefined) throw new LuaError(`attempt to perform arithmetic on a ${luaType(l)} value`);
  if (b === undefined) throw new LuaError(`attempt to perform arithmetic on a ${luaType(r)} value`);
  return fn(a, b);
}

function luaEquals(l, r) {
  if (l === undefined && r === undefined) return true;
  return l === r;
}

function compare(l, r) {
  if (typeof l === 'number' && typeof r === 'number') return l - r;
  if (typeof l === 'string' && typeof r === 'string') return l < r ? -1 : l > r ? 1 : 0;
  throw new LuaError(`attempt to compare ${luaType(l)} with ${luaType(r)}`);
}

// Minimal string.format supporting %d %i %s %f %.Nf %x %% %q
function luaFormat(fmt, args) {
  let idx = 0;
  return String(fmt).replace(/%%|%[-+ #0]*\d*(?:\.\d+)?[diouxXeEfgGqsc]/g, (spec) => {
    if (spec === '%%') return '%';
    const arg = args[idx++];
    const conv = spec[spec.length - 1];
    const precMatch = spec.match(/\.(\d+)/);
    const prec = precMatch ? parseInt(precMatch[1], 10) : null;
    switch (conv) {
      case 'd': case 'i': case 'u': return String(Math.trunc(tonumber(arg) || 0));
      case 'x': return (tonumber(arg) >>> 0).toString(16);
      case 'X': return (tonumber(arg) >>> 0).toString(16).toUpperCase();
      case 'o': return (tonumber(arg) >>> 0).toString(8);
      case 'f': case 'F': case 'g': case 'G': case 'e': case 'E': {
        const num = tonumber(arg) || 0;
        return prec !== null ? num.toFixed(prec) : String(num);
      }
      case 's': {
        const s = luaToString(arg);
        return prec !== null ? s.substring(0, prec) : s;
      }
      case 'q': return '"' + luaToString(arg).replace(/["\\\n]/g, (m) => ({ '"': '\\"', '\\': '\\\\', '\n': '\\n' }[m])) + '"';
      case 'c': return String.fromCharCode(tonumber(arg));
      default: return spec;
    }
  });
}

// Convenience: convert a Lua value to a plain JS value (tables → objects/arrays).
export function luaToJS(v) {
  if (v instanceof LuaTable) {
    const len = v.length();
    // Heuristic: pure array
    let isArray = len > 0;
    for (const k of v.hash.keys()) {
      if (typeof k !== 'number' || k < 1 || k > len || !Number.isInteger(k)) { isArray = false; break; }
    }
    if (isArray) {
      const arr = [];
      for (let i = 1; i <= len; i++) arr.push(luaToJS(v.get(i)));
      return arr;
    }
    const obj = {};
    for (const [k, val] of v.hash) obj[k] = luaToJS(val);
    return obj;
  }
  return v;
}
