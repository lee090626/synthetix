const __page = __PAGE_JSON__;
const __contracts = __CONTRACTS__;
const __externs = __EXTERNS__;

let __exports = null;
let __state = {};
let __lastError = null;
// Dev server build error; while set it owns the error overlay over application errors.
let __buildError = null;
// Set to the load error when stored state cannot be loaded; saving would overwrite the user's data.
let __persistenceBlocked = null;
let __lastTransition = null;
let __lastEffect = null;
let __effectAdapter = null;
let __nextEffectId = 0;
let __latestEffectId = 0;
const __effectRuns = new Map();
const __effectRetries = new Map();
let __storageMigration = typeof window.synthetixStorageMigration === 'function'
  ? window.synthetixStorageMigration
  : null;
const __storagePrefix = `synthetix:${__page.name}:`;
const __storageKey = `${__storagePrefix}state`;

function getPath(scope, path) {
  return path.split('.').reduce((value, part) => value?.[part], scope);
}

function setPath(scope, path, value) {
  const parts = path.split('.');
  if (parts.length === 1) {
    scope[parts[0]] = value;
    return;
  }
  const owner = getPath(scope, parts.slice(0, -1).join('.'));
  if (!owner) throw new Error(`Missing state path: ${path}`);
  owner[parts.at(-1)] = value;
}

function evaluate(expression, scope = __state) {
  switch (expression.kind) {
    case 'number': return expression.value >>> 0;
    case 'i32': return expression.value | 0;
    case 'bool': return expression.value;
    case 'text': return expression.value;
    case 'ident': return scope[expression.name];
    case 'field': return evaluate(expression.object, scope)?.[expression.name];
    case 'list': return expression.values.map((value) => evaluate(value, scope));
    case 'record': return Object.fromEntries(
      Object.entries(expression.fields).map(([name, value]) => [name, evaluate(value, scope)]),
    );
    case 'let': {
      const nested = { ...scope, [expression.name]: evaluate(expression.value, scope) };
      return evaluate(expression.body, nested);
    }
    case 'if': return evaluate(expression.condition, scope)
      ? evaluate(expression.then, scope)
      : evaluate(expression.else, scope);
    case 'binary': return binary(expression.op, evaluate(expression.left, scope), evaluate(expression.right, scope));
    default: throw new Error(`Unknown expression kind: ${expression.kind}`);
  }
}

function binary(op, left, right) {
  switch (op) {
    case 'add': return (left + right) >>> 0;
    case 'sub': return (left - right) >>> 0;
    case 'sub_checked':
      if (left < right) throw new Error('subtraction below 0');
      return left - right;
    case 'mul': return Math.imul(left, right) >>> 0;
    case 'div':
      if (right === 0) throw new Error('division by zero');
      return Math.floor(left / right) >>> 0;
    case 'rem':
      if (right === 0) throw new Error('division by zero');
      return (left % right) >>> 0;
    case 'shl': return (left << (right & 31)) >>> 0;
    case 'eq': return left === right;
    case 'ne': return left !== right;
    case 'lt': return left < right;
    case 'le': return left <= right;
    case 'gt': return left > right;
    case 'ge': return left >= right;
    case 'and': return left && right;
    case 'or': return left || right;
    default: throw new Error(`Unknown operator: ${op}`);
  }
}

function decimalsOf(name) {
  return __page.states.find((candidate) => candidate.name === name)?.decimals ?? 0;
}

// A value with decimals is stored times 10^places: 1250 with 2 places is 12.50.
function display(value, decimals = 0) {
  if (value === undefined) return '';
  if (decimals > 0 && typeof value === 'number') {
    const sign = value < 0 ? '-' : '';
    const digits = String(Math.abs(value)).padStart(decimals + 1, '0');
    return `${sign}${digits.slice(0, -decimals)}.${digits.slice(-decimals)}`;
  }
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

function applicationError(error, functionName = null, sourceOverride = null) {
  if (error?.category === 'APPLICATION_RUNTIME_ERROR' || error?.category === 'APPLICATION_TRANSITION_FAILURE') return error;
  const contract = functionName ? __contracts[functionName] : null;
  const source = sourceOverride ?? contract?.source ?? __page.source;
  const message = error instanceof Error ? error.message : String(error);
  const location = functionName ? `${source}:${functionName}` : source;
  const wrapped = new Error(`${location}: ${message}`);
  wrapped.category = 'APPLICATION_RUNTIME_ERROR';
  wrapped.source = source;
  wrapped.contract = functionName;
  return wrapped;
}

// The overlay shows one error, by priority: build error, then the last application
// error (a blocked store re-reports itself on clear), then nothing.
function renderErrorOverlay() {
  const node = document.querySelector('#synthetix-error');
  delete node.dataset.dev;
  delete node.dataset.category;
  delete node.dataset.source;
  if (__buildError) {
    node.hidden = false;
    node.textContent = __buildError;
    node.dataset.dev = 'build';
  } else if (__lastError) {
    node.hidden = false;
    node.textContent = __lastError.message;
    node.dataset.category = __lastError.category;
    node.dataset.source = __lastError.source;
  } else {
    node.hidden = true;
    node.textContent = '';
  }
}

function reportApplicationError(error, functionName = null, sourceOverride = null) {
  __lastError = applicationError(error, functionName, sourceOverride);
  renderErrorOverlay();
  return __lastError;
}

function clearApplicationError() {
  if (__persistenceBlocked) {
    reportApplicationError(__persistenceBlocked);
    return;
  }
  __lastError = null;
  renderErrorOverlay();
}

function runApplicationAction(action) {
  try {
    const result = action();
    clearApplicationError();
    return result;
  } catch (error) {
    return reportApplicationError(error);
  }
}

function lowerArgument(value, type, index) {
  if (type === 'bool') {
    if (typeof value !== 'boolean') throw new TypeError(`argument ${index}: expected bool`);
    return value ? 1 : 0;
  }
  if (type === 'u32') {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > 0xffffffff)
      throw new TypeError(`argument ${index}: expected u32`);
    return value;
  }
  if (type === 'i32') {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < -0x80000000 || value > 0x7fffffff)
      throw new TypeError(`argument ${index}: expected i32`);
    return value;
  }
  throw new TypeError(`argument ${index}: unsupported verified type ${type}`);
}

function liftResult(value, type) {
  if (type === 'bool') {
    if (value !== 0 && value !== 1) throw new TypeError(`verified WASM returned invalid bool ${value}`);
    return value === 1;
  }
  if (type === 'u32') return value >>> 0;
  if (type === 'i32') return value | 0;
  throw new TypeError(`unsupported verified result type ${type}`);
}

function isApplicationValue(value, seen = new Set()) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return true;
  if (typeof value === 'number') return Number.isSafeInteger(value);
  if (typeof value !== 'object' || seen.has(value)) return false;
  if (!Array.isArray(value)) {
    const prototype = Object.getPrototypeOf(value);
    if (Object.prototype.toString.call(value) !== '[object Object]'
      || (prototype !== null && prototype.constructor?.name !== 'Object')) return false;
  }
  seen.add(value);
  const children = Array.isArray(value) ? value : Object.values(value);
  for (const child of children) {
    if (!isApplicationValue(child, seen)) return false;
  }
  seen.delete(value);
  return true;
}

function validateEffectResponse(value, type = null) {
  if (type === 'bool' && typeof value === 'boolean') return value;
  if (type === 'u32' && typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 0xffffffff) return value;
  if (type === 'i32' && typeof value === 'number' && Number.isInteger(value) && value >= -0x80000000 && value <= 0x7fffffff) return value;
  if (type !== null && type !== undefined) throw new TypeError(`external response does not match ${type}`);
  if (!isApplicationValue(value)) throw new TypeError('external response is not a supported application value');
  return value;
}

// A text given to a type's input is the type's value; its place is passed on.
// This mapping is not verified: the rule is proven for every place.
function choicePlace(contract, value, index) {
  const choice = contract.choices?.[index];
  if (!choice || typeof value !== 'string') return value;
  const place = choice.values.indexOf(value);
  if (place < 0)
    throw new TypeError(`argument ${index + 1}: "${value}" is not a value of ${choice.name} (${choice.values.join(', ')})`);
  return place;
}

function callContract(name, args) {
  try {
    const contract = __contracts[name];
    if (!contract) throw new Error(`Missing verified contract: ${name}`);
    if (!Array.isArray(args) || args.length !== contract.params.length)
      throw new TypeError(`${name}: expected ${contract.params.length} arguments`);
    const fn = __exports?.[name];
    if (typeof fn !== 'function') throw new Error(`Missing WASM export: ${name}`);
    const lowered = args.map((value, index) =>
      lowerArgument(choicePlace(contract, value, index), contract.params[index], index));
    if (contract.requires) {
      const check = __exports?.[contract.requires];
      if (typeof check !== 'function') throw new Error(`Missing WASM precondition check: ${name}`);
      if (check(...lowered) !== 1)
        throw new Error(['입력이 이 조건에 맞지 않습니다:', ...(contract.needs ?? [])].join('\n  '));
    }
    const result = liftResult(fn(...lowered), contract.result);
    if (!contract.labels) return result;
    if (!(result in contract.labels)) throw new TypeError(`${name}: no text for answer ${result}`);
    return contract.labels[result];
  } catch (error) {
    throw applicationError(error, name);
  }
}

function parseInputValue(input, type, decimals = 0) {
  if (type === 'text') return input.value;
  if (type === 'bool') return input.checked;
  if (type !== 'u32' && type !== 'i32') throw new TypeError(`input does not support ${type}`);
  if (input.value.trim() === '') throw new TypeError(`expected ${type} input`);
  if (decimals > 0) {
    const match = /^(-?)(\d+)(?:\.(\d+))?$/.exec(input.value.trim());
    if (!match || (match[3] ?? '').length > decimals || (match[1] && type !== 'i32'))
      throw new TypeError(`expected a number with at most ${decimals} decimal places`);
    const scaled = Number(match[1] + match[2] + (match[3] ?? '').padEnd(decimals, '0'));
    const minimum = type === 'i32' ? -0x80000000 : 0;
    const maximum = type === 'i32' ? 0x7fffffff : 0xffffffff;
    if (!Number.isSafeInteger(scaled) || scaled < minimum || scaled > maximum)
      throw new TypeError(`input is outside ${type}`);
    return scaled;
  }
  const value = Number(input.value);
  const minimum = type === 'i32' ? -0x80000000 : 0;
  const maximum = type === 'i32' ? 0x7fffffff : 0xffffffff;
  if (!Number.isInteger(value) || value < minimum || value > maximum)
    throw new TypeError(`input is outside ${type}`);
  return value;
}

function storageAdapter() {
  const adapter = window.synthetixStorageAdapter ?? localStorage;
  if (!adapter || typeof adapter.getItem !== 'function' || typeof adapter.setItem !== 'function')
    throw new TypeError('storage adapter must implement getItem and setItem');
  return adapter;
}

function validatePersistedValues(values) {
  if (!values || typeof values !== 'object' || Array.isArray(values))
    throw new TypeError('persisted state must be an object');
  for (const [name, value] of Object.entries(values)) {
    const state = __page.states.find((candidate) => candidate.name === name && candidate.persisted);
    if (!state) throw new TypeError(`unknown persisted state: ${name}`);
    const valid = state.type === 'bool' ? typeof value === 'boolean'
      : state.type === 'u32' ? Number.isInteger(value) && value >= 0 && value <= 0xffffffff
      : state.type === 'i32' ? Number.isInteger(value) && value >= -0x80000000 && value <= 0x7fffffff
      : state.type === 'text' ? typeof value === 'string'
      : state.type === 'list' ? Array.isArray(value)
      : state.type === 'record' ? value !== null && typeof value === 'object' && !Array.isArray(value)
      : false;
    if (!valid) throw new TypeError(`persisted state ${name} does not match ${state.type}`);
  }
  return values;
}

function saveState(state = __state) {
  const persisted = __page.states.filter((candidate) => candidate.persisted);
  if (persisted.length === 0 || __persistenceBlocked) return;
  const values = Object.fromEntries(persisted.map((candidate) => [candidate.name, state[candidate.name]]));
  validatePersistedValues(values);
  storageAdapter().setItem(__storageKey, JSON.stringify({
    version: __page.storageSchemaVersion,
    values,
  }));
}

async function loadPersistedState(baseState = __state) {
  const persisted = __page.states.filter((candidate) => candidate.persisted);
  if (persisted.length === 0) return baseState;
  const adapter = storageAdapter();
  const stored = adapter.getItem(__storageKey);
  let values;
  let version;
  let legacy = false;
  if (stored === null) {
    values = {};
    for (const state of persisted) {
      const saved = adapter.getItem(__storagePrefix + state.name);
      if (saved !== null) {
        values[state.name] = JSON.parse(saved);
        legacy = true;
      }
    }
    if (!legacy) return baseState;
    version = 0;
  } else {
    const envelope = JSON.parse(stored);
    if (!envelope || typeof envelope !== 'object' || Array.isArray(envelope)
      || !Number.isSafeInteger(envelope.version) || envelope.version < 0
      || !Object.prototype.hasOwnProperty.call(envelope, 'values'))
      throw new TypeError('persisted state envelope is malformed');
    ({ version, values } = envelope);
  }

  let migrated = false;
  if (version !== __page.storageSchemaVersion) {
    if (legacy && version === 0 && !__storageMigration) {
      migrated = true;
    } else if (__storageMigration) {
      values = await __storageMigration({
        page: __page.name,
        fromVersion: version,
        toVersion: __page.storageSchemaVersion,
        values: structuredClone(values),
      });
      migrated = true;
    } else {
      throw new Error(`incompatible storage schema ${version}; expected ${__page.storageSchemaVersion}`);
    }
  }

  validatePersistedValues(values);
  const nextState = structuredClone(baseState);
  Object.assign(nextState, values);
  if (migrated) saveState(nextState);
  return nextState;
}

function callVerified(element, scope, target) {
  setPath(scope, target, callContract(element.function, element.args.map((arg) => evaluate(arg, scope))));
  saveState();
  render();
}

// Table rows and rows cleared are page bookkeeping, not rules. Returns false otherwise.
function applyTableUpdate(update, nextState) {
  if (update.function === '__append') {
    getPath(nextState, update.state).push(evaluate(update.args[0], nextState));
    return true;
  }
  if (update.function === '__empty') {
    setPath(nextState, update.state, []);
    return true;
  }
  return false;
}

// Effects are host calls, not verified rules: they are only reachable from
// page actions, and their answers are checked by the declared type.
function isExtern(name) {
  return Object.hasOwn(__externs, name);
}

function failTransition(element, updateIndex, functionName, error) {
  const cause = applicationError(error, functionName);
  const failure = new Error(cause.message);
  failure.category = 'APPLICATION_TRANSITION_FAILURE';
  failure.source = cause.source;
  failure.contract = cause.contract;
  failure.event = element.event;
  failure.failedUpdate = updateIndex;
  failure.cause = cause;
  __lastTransition = {
    event: element.event,
    status: 'FAILED',
    updatesApplied: 0,
    failedUpdate: updateIndex,
    state: structuredClone(__state),
    error: failure,
  };
  reportApplicationError(failure);
  return __lastTransition;
}

function commitTransition(element, nextState) {
  for (const key of Object.keys(__state)) delete __state[key];
  Object.assign(__state, nextState);
  __lastTransition = {
    event: element.event,
    status: 'APPLIED',
    updatesApplied: element.updates.length,
    state: structuredClone(__state),
  };
  try {
    render();
    clearApplicationError();
  } catch (error) {
    reportApplicationError(error);
  }
  return __lastTransition;
}

function applyReducerTransition(element) {
  if (element.updates.some((update) => isExtern(update.function))) return applyEffectTransition(element);
  const nextState = structuredClone(__state);
  let updateIndex = 0;
  let functionName = null;
  try {
    for (; updateIndex < element.updates.length; updateIndex++) {
      const update = element.updates[updateIndex];
      if (applyTableUpdate(update, nextState)) continue;
      functionName = update.function;
      const value = callContract(update.function, update.args.map((arg) => evaluate(arg, nextState)));
      setPath(nextState, update.state, value);
    }
    functionName = null;
    saveState(nextState);
  } catch (error) {
    return failTransition(element, updateIndex, functionName, error);
  }
  return commitTransition(element, nextState);
}

// Like applyReducerTransition, for updates that call an effect. Each answer is
// awaited before the next update reads it. The state changes only when every
// update succeeds, and then only the paths the transition sets are written, so
// changes made by other clicks while the effect was pending are kept.
// A superseded or cancelled effect resolves to undefined and changes nothing.
async function applyEffectTransition(element) {
  const nextState = structuredClone(__state);
  const written = [];
  let updateIndex = 0;
  let functionName = null;
  try {
    for (; updateIndex < element.updates.length; updateIndex++) {
      const update = element.updates[updateIndex];
      written.push(update.state);
      if (applyTableUpdate(update, nextState)) continue;
      functionName = update.function;
      const args = update.args.map((arg) => evaluate(arg, nextState));
      const value = isExtern(update.function)
        ? await window.synthetix.requestExtern(update.function, args)
        : callContract(update.function, args);
      if (value === undefined) {
        __lastTransition = {
          event: element.event,
          status: 'CANCELLED',
          updatesApplied: 0,
          failedUpdate: updateIndex,
          state: structuredClone(__state),
        };
        return __lastTransition;
      }
      setPath(nextState, update.state, value);
    }
    functionName = null;
    const committed = structuredClone(__state);
    for (const path of written) setPath(committed, path, getPath(nextState, path));
    saveState(committed);
    return commitTransition(element, committed);
  } catch (error) {
    return failTransition(element, updateIndex, functionName, error);
  }
}

function renderElement(element, root, scope) {
  if (element.kind === 'show') {
    const output = document.createElement('output');
    output.dataset.state = element.state;
    output.textContent = display(getPath(scope, element.state), decimalsOf(element.state));
    root.append(output);
    return;
  }
  if (element.kind === 'show-expr') {
    const output = document.createElement('output');
    output.textContent = display(evaluate(element.value, scope), element.decimals ?? 0);
    root.append(output);
    return;
  }
  if (element.kind === 'text') {
    const text = document.createElement('p');
    text.textContent = element.text;
    root.append(text);
    return;
  }
  if (element.kind === 'when-show') {
    if (evaluate(element.condition, scope)) {
      const message = document.createElement('p');
      message.textContent = element.text;
      root.append(message);
    }
    return;
  }
  if (element.kind === 'input' && element.options) {
    // A text state a rule takes as a type: a box of the type's values.
    const select = document.createElement('select');
    const blank = document.createElement('option');
    blank.value = '';
    blank.textContent = element.placeholder;
    select.append(blank);
    for (const value of element.options) {
      const option = document.createElement('option');
      option.value = value;
      option.textContent = value;
      select.append(option);
    }
    select.value = getPath(__state, element.state) ?? '';
    select.addEventListener('change', () => {
      setPath(__state, element.state, select.value);
      saveState();
      clearApplicationError();
    });
    root.append(select);
    return;
  }
  if (element.kind === 'input') {
    const state = __page.states.find((candidate) => candidate.name === element.state);
    const input = document.createElement('input');
    input.type = state?.type === 'bool' ? 'checkbox' : state?.type === 'u32' || state?.type === 'i32' ? 'number' : 'text';
    input.step = input.type === 'number' ? (state?.decimals ? String(10 ** -state.decimals) : '1') : undefined;
    if (input.type === 'checkbox') input.checked = getPath(__state, element.state) ?? false;
    else input.value = display(getPath(__state, element.state), state?.decimals ?? 0);
    input.placeholder = element.placeholder;
    input.addEventListener('change', () => {
      try {
        setPath(__state, element.state, parseInputValue(input, state?.type ?? 'unsupported', state?.decimals ?? 0));
        saveState();
        clearApplicationError();
      } catch (error) {
        reportApplicationError(error);
      }
    });
    root.append(input);
    return;
  }
  if (element.kind === 'list') {
    const list = document.createElement('ul');
    for (const item of getPath(scope, element.state) ?? []) {
      const row = document.createElement('li');
      const rowScope = { ...scope, [element.item]: item };
      for (const child of element.elements) renderElement(child, row, rowScope);
      list.append(row);
    }
    root.append(list);
    return;
  }
  if (element.kind === 'button') {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = element.label;
    button.addEventListener('click', () => {
      // An effect button runs as a one-update transition, so its result is reported like a reducer's.
      if (isExtern(element.function)) {
        applyReducerTransition({
          event: element.label,
          updates: [{ state: element.state, function: element.function, args: element.args }],
        });
        return;
      }
      try { callVerified(element, scope, element.state); clearApplicationError(); }
      catch (error) { reportApplicationError(error, element.function); }
    });
    root.append(button);
    return;
  }
  if (element.kind === 'transfers') {
    // "name → name amount" for every pair that sends money; copy and keep past results.
    const size = element.names.length;
    const lines = [];
    element.sends.forEach((state, index) => {
      const amount = getPath(scope, state) ?? 0;
      if (amount <= 0) return;
      const from = getPath(scope, element.names[Math.floor(index / size)]) || `${Math.floor(index / size) + 1}`;
      const to = getPath(scope, element.names[index % size]) || `${(index % size) + 1}`;
      lines.push(`${from} → ${to} ${display(amount, decimalsOf(state))}`);
    });
    const text = lines.join('\n');
    for (const line of lines) {
      const row = document.createElement('p');
      row.textContent = line;
      root.append(row);
    }
    const historyKey = `${__storagePrefix}history`;
    const readHistory = () => {
      try { return JSON.parse(storageAdapter().getItem(historyKey) ?? '[]'); } catch { return []; }
    };
    const copy = document.createElement('button');
    copy.type = 'button';
    copy.textContent = '복사';
    copy.addEventListener('click', () => { navigator.clipboard?.writeText(text); });
    const keep = document.createElement('button');
    keep.type = 'button';
    keep.textContent = '기록 저장';
    keep.addEventListener('click', () => {
      if (!text) return;
      const history = readHistory();
      history.unshift({ at: new Date().toISOString().slice(0, 10), text });
      storageAdapter().setItem(historyKey, JSON.stringify(history.slice(0, 50)));
      render();
    });
    root.append(copy, keep);
    for (const entry of readHistory()) {
      const past = document.createElement('pre');
      past.textContent = `${entry.at}\n${entry.text}`;
      root.append(past);
    }
    return;
  }
  if (element.kind === 'add') {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = element.label;
    button.addEventListener('click', () => {
      try {
        const list = getPath(__state, element.state);
        if (!Array.isArray(list)) throw new Error(`State is not a list: ${element.state}`);
        list.push(evaluate(element.value, __state));
        saveState();
        render();
        clearApplicationError();
      } catch (error) { reportApplicationError(error); }
    });
    root.append(button);
    return;
  }
  if (element.kind === 'reducer') {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = element.label;
    button.addEventListener('click', () => applyReducerTransition(element));
    root.append(button);
  }
}

function render() {
  const root = document.querySelector('#app');
  root.replaceChildren();
  const title = document.createElement('h1');
  title.textContent = __page.name;
  root.append(title);
  for (const element of __page.elements) renderElement(element, root, __state);
}

async function boot() {
  const runtimeConfig = window.synthetixConfig ?? {};
  if (typeof runtimeConfig !== 'object' || runtimeConfig === null
    || Array.isArray(runtimeConfig) || !isApplicationValue(runtimeConfig))
    throw new TypeError('deployment config must be a JSON object');
  const bytes = await fetch('./app.wasm').then((response) => response.arrayBuffer());
  const { instance } = await WebAssembly.instantiate(bytes, {});
  __exports = instance.exports;
  for (const state of __page.states) __state[state.name] = evaluate(state.initial);
  try { Object.assign(__state, await loadPersistedState(__state)); }
  catch (error) {
    __persistenceBlocked = new Error(`${error.message}; stored data was kept and will not be overwritten until it is restored or migrated`);
    reportApplicationError(__persistenceBlocked);
  }
  window.synthetix = {
    state: __state,
    config: Object.freeze({ ...runtimeConfig }),
    call: callContract,
    storageSchemaVersion: __page.storageSchemaVersion,
    registerStorageMigration(migration) {
      if (typeof migration !== 'function') throw reportApplicationError(new TypeError('storage migration must be a function'));
      __storageMigration = migration;
    },
    async restore() {
      const wasBlocked = __persistenceBlocked;
      __persistenceBlocked = null;
      try {
        const restored = await loadPersistedState(__state);
        for (const key of Object.keys(__state)) delete __state[key];
        Object.assign(__state, restored);
        render();
        clearApplicationError();
        return __state;
      } catch (error) {
        __persistenceBlocked = wasBlocked;
        throw reportApplicationError(error);
      }
    },
    registerEffectAdapter(adapter) {
      if (typeof adapter !== 'function') throw reportApplicationError(new TypeError('effect adapter must be a function'));
      __effectAdapter = adapter;
    },
    cancelEffect(target = null) {
      let cancelled = false;
      for (const run of __effectRuns.values()) {
        if (run.status !== 'LOADING'
          || (target !== null && run.targetState !== target && run.name !== target)) continue;
        run.status = 'CANCELLED';
        run.controller.abort();
        if (__latestEffectId === run.id)
          __lastEffect = { name: run.name, status: 'CANCELLED', requestId: run.id, targetState: run.targetState };
        cancelled = true;
      }
      return cancelled;
    },
    retryEffect(target = null) {
      const latest = __lastEffect;
      const key = target === null
        ? latest?.targetState !== null && latest?.targetState !== undefined
          ? `state:${latest.targetState}`
          : `effect:${latest?.name}`
        : `state:${target}`;
      let retry = __effectRetries.get(key);
      if (!retry && target !== null)
        retry = [...__effectRetries.values()].find((candidate) => candidate.request.name === target);
      if (!retry) throw reportApplicationError(new Error('No failed effect request to retry'));
      return window.synthetix.requestEffect(retry.request, retry.targetState);
    },
    async requestExtern(name, args, targetState = null) {
      let requestId = null;
      try {
        const declaration = __externs[name];
        if (!declaration) throw new Error(`Missing extern declaration: ${name}`);
        if (!Array.isArray(args) || args.length !== declaration.params.length)
          throw new TypeError(`${name}: expected ${declaration.params.length} arguments`);
        args.forEach((value, index) => lowerArgument(value, declaration.params[index], index));
        if (targetState !== null) {
          const state = __page.states.find((candidate) => candidate.name === targetState);
          if (!state || state.type !== declaration.result)
            throw new TypeError(`${name}: target state must have type ${declaration.result}`);
        }
        const pending = window.synthetix.requestEffect({
          name,
          source: declaration.source,
          inputs: args,
          effect: declaration.effect,
          verificationStatus: declaration.verification_status,
          trustBoundary: declaration.trust_boundary,
          responseType: declaration.result,
        }, targetState);
        requestId = __lastEffect?.requestId ?? null;
        return await pending;
      } catch (error) {
        const failure = reportApplicationError(error, null, __externs[name]?.source);
        if (requestId === null) __lastEffect = { name, status: 'ERROR', error: failure };
        else if (__lastEffect?.requestId === requestId)
          __lastEffect = { ...__lastEffect, name, status: 'ERROR', error: failure };
        throw failure;
      }
    },
    async requestEffect(request, targetState = null) {
      const name = typeof request?.name === 'string' ? request.name : null;
      const key = targetState === null ? `effect:${name}` : `state:${targetState}`;
      const previous = __effectRuns.get(key);
      if (previous?.status === 'LOADING') {
        previous.status = 'STALE';
        previous.controller.abort();
      }
      const controller = new AbortController();
      const run = { id: ++__nextEffectId, key, name, targetState, controller, status: 'LOADING' };
      __effectRuns.set(key, run);
      __effectRetries.delete(key);
      __latestEffectId = run.id;
      __lastEffect = { name, status: 'LOADING', requestId: run.id, targetState };
      let retryRequest = null;
      try {
        if (!request || typeof request.name !== 'string' || !Array.isArray(request.inputs))
          throw new TypeError('effect request must contain a name and inputs');
        if (!request.inputs.every((value) => isApplicationValue(value)))
          throw new TypeError('effect request contains an unsupported input value');
        if (!__effectAdapter) throw new Error(`No effect adapter registered for ${request.name}`);
        retryRequest = { ...request, inputs: JSON.parse(JSON.stringify(request.inputs)) };
        const signal = controller.signal;
        let abortListener;
        const aborted = new Promise((_, reject) => {
          abortListener = () => {
            const error = new Error('effect request cancelled');
            error.name = 'AbortError';
            reject(error);
          };
          signal.addEventListener('abort', abortListener, { once: true });
          if (signal.aborted) abortListener();
        });
        let response;
        try {
          response = await Promise.race([
            Promise.resolve().then(() => __effectAdapter(request, { signal, requestId: run.id })),
            aborted,
          ]);
        } finally {
          signal.removeEventListener('abort', abortListener);
        }
        if (signal.aborted || __effectRuns.get(key) !== run) return undefined;
        const result = validateEffectResponse(response, request.responseType ?? null);
        if (targetState !== null) {
          setPath(__state, targetState, result);
          saveState();
          render();
        }
        run.status = 'SUCCEEDED';
        if (__latestEffectId === run.id) {
          __lastEffect = { name, status: 'SUCCEEDED', response: result, requestId: run.id, targetState };
          clearApplicationError();
        }
        return result;
      } catch (error) {
        if (__effectRuns.get(key) !== run) return undefined;
        if (controller.signal.aborted || error?.name === 'AbortError') {
          run.status = 'CANCELLED';
          if (__latestEffectId === run.id)
            __lastEffect = { name, status: 'CANCELLED', requestId: run.id, targetState };
          return undefined;
        }
        const failure = reportApplicationError(error, null, request?.source);
        run.status = 'ERROR';
        if (retryRequest) __effectRetries.set(key, { request: retryRequest, targetState });
        if (__latestEffectId === run.id)
          __lastEffect = { name, status: 'ERROR', error: failure, requestId: run.id, targetState };
        throw failure;
      }
    },
    get lastError() { return __lastError; },
    get lastTransition() { return __lastTransition; },
    get lastEffect() { return __lastEffect; },
    // 'BLOCKED' while stored data could not be loaded and saving is paused.
    get persistence() { return __persistenceBlocked ? 'BLOCKED' : 'OK'; },
  };
  render();
  return window.synthetix;
}

// Resolves with window.synthetix once the app runs; rejects if it failed to start.
window.synthetixReady = boot();
window.synthetixReady.catch((error) => {
  reportApplicationError(error);
});

