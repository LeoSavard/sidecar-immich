// SPDX-License-Identifier: Apache-2.0
// Local accounts end to end (crates/api/src/accounts.rs), against a server with no Immich, sign-in on and no
// accounts yet: first-run setup with the code from the server's log, adding someone, their sign-in, a reset
// that signs them out, a changed password, the lockout, the last administrator, removal, sign-out.
//
//   node web/scripts/signin-smoke.mjs <base-url> <setup code>
//
// Run inside the dev container with --network host, against `./dev/serve` started with a config that has no
// `[sign_in] enabled = false`. It makes accounts; delete `accounts/` and `sessions/` in that state directory
// afterwards.
import { chromium } from 'playwright';

const [base, code] = process.argv.slice(2);
if (!base || !code) throw new Error('usage: signin-smoke.mjs <base-url> <setup code>');

const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
const owner = await browser.newContext();
const kid = await browser.newContext();
const stranger = await browser.newContext();
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};
const call = (ctx, method, path, body) =>
	ctx.request.fetch(base + path, { method, headers: { 'content-type': 'application/json' }, data: body === undefined ? undefined : JSON.stringify(body) });

// 1. A fresh server asks for setup, and only with its code.
let m = await (await owner.request.get(base + '/api/sign-in')).json();
check(m.method === 'password' && m.setup === true, 'a fresh server asks for its first account');
const page = await owner.newPage();
await page.goto(base + '/');
await page.waitForURL(/\/signin/);
await page.getByLabel('Setup code').fill('0000-0000');
await page.getByLabel('Your name').fill('The Owner');
await page.getByLabel('Username').fill('Owner');
await page.getByLabel('Password', { exact: true }).fill('correct horse battery');
await page.getByLabel('Password again').fill('correct horse battery');
await page.getByRole('button', { name: 'Create the account' }).click();
await page.getByRole('alert').waitFor();
check((await page.getByRole('alert').textContent()).includes('setup code'), 'a wrong setup code is refused');
await page.getByLabel('Setup code').fill(code.toLowerCase());
await page.getByRole('button', { name: 'Create the account' }).click();
await page.waitForURL((u) => !u.pathname.startsWith('/signin'));
let who = await (await owner.request.get(base + '/api/session')).json();
check(who.isAdmin && who.name === 'The Owner' && who.authRequired && who.signIn === 'password', 'the first account is an administrator, signed in');
m = await (await owner.request.get(base + '/api/sign-in')).json();
check(m.setup === false, 'setup is over once an account exists');
check((await call(owner, 'POST', '/api/accounts/setup', { code, username: 'again', password: 'another password' })).status() === 409, 'setup cannot run twice');

// 2. Settings: add someone.
await page.goto(base + '/settings');
await page.getByRole('heading', { name: 'People' }).waitFor();
await page.getByRole('button', { name: 'Add someone' }).click();
await page.getByPlaceholder('Name', { exact: true }).fill('Kid');
await page.getByPlaceholder('Username').fill('kid');
await page.getByPlaceholder('Password (8 characters or more)', { exact: true }).fill('kid password one');
await page.getByRole('button', { name: 'Add', exact: true }).click();
await page.getByText('Account added').waitFor();
check(await page.getByText('kid', { exact: true }).isVisible(), 'an administrator adds someone');
check((await call(owner, 'POST', '/api/accounts', { username: 'KID', password: 'whatever long' })).status() === 400, 'usernames are unique, whatever their case');

// 3. They sign in, and are not an administrator.
const kp = await kid.newPage();
await kp.goto(base + '/signin');
await kp.getByLabel('Username').fill('kid');
await kp.getByLabel('Password', { exact: true }).fill('kid password one');
await kp.getByRole('button', { name: 'Sign in' }).click();
await kp.waitForURL((u) => !u.pathname.startsWith('/signin'));
who = await (await kid.request.get(base + '/api/session')).json();
check(who.name === 'Kid' && !who.isAdmin, 'they sign in, not as an administrator');
check((await kid.request.get(base + '/api/accounts')).status() === 403, 'and cannot manage accounts');
await kp.goto(base + '/settings');
await kp.getByRole('heading', { name: 'Your account' }).waitFor();
check((await kp.getByRole('heading', { name: 'People' }).count()) === 0, 'their Settings has no People');

// 4. They change their own password: still signed in here.
await kp.getByPlaceholder('Current password').fill('kid password one');
await kp.getByPlaceholder('New password (8 characters or more)').fill('kid password two');
await kp.getByPlaceholder('New password again').fill('kid password two');
await kp.getByRole('button', { name: 'Change password' }).click();
await kp.getByText('Password changed').waitFor();
check((await kid.request.get(base + '/api/session')).ok(), 'changing your own password keeps this browser signed in');

// 5. The administrator sets a new password: they are signed out everywhere.
await page.reload();
const row = page.getByRole('listitem').filter({ hasText: 'Kid' });
await row.getByRole('button', { name: 'Set password' }).click();
await row.getByPlaceholder(/New password for/).fill('kid password three');
await row.getByRole('button', { name: 'Set', exact: true }).click();
await page.getByText('signed out everywhere').waitFor();
check((await kid.request.get(base + '/api/session')).status() === 401, 'a reset signs them out');
check((await call(kid, 'POST', '/api/session', { username: 'kid', password: 'kid password two' })).status() === 401, 'the old password no longer works');
check((await call(kid, 'POST', '/api/session', { username: 'kid', password: 'kid password three' })).ok(), 'the new one does');

// 6. Guessing is slowed down, for names that exist and names that don't alike.
const statuses = [];
for (let i = 0; i < 6; i++) statuses.push((await call(stranger, 'POST', '/api/session', { username: 'nobody', password: 'guess ' + i })).status());
check(statuses.slice(0, 5).every((s) => s === 401) && statuses[5] === 429, `the sixth wrong guess has to wait (${statuses})`);

// 7. Someone stays an administrator; nobody removes themselves.
const me = (await (await owner.request.get(base + '/api/session')).json()).id;
check((await call(owner, 'PUT', `/api/accounts/${me}`, { isAdmin: false })).status() === 400, 'the last administrator stays one');
check((await call(owner, 'DELETE', `/api/accounts/${me}`)).status() === 400, 'nobody removes their own account');

// 8. Removing them signs them out for good.
page.once('dialog', (d) => d.accept());
await page.reload();
await page.getByRole('listitem').filter({ hasText: 'Kid' }).getByRole('button', { name: 'Remove' }).click();
await page.getByText("account is removed").waitFor();
check((await kid.request.get(base + '/api/session')).status() === 401, 'a removed account is signed out');
check((await call(kid, 'POST', '/api/session', { username: 'kid', password: 'kid password three' })).status() === 401, 'and cannot sign in');

// 9. Sign out.
await page.goto(base + '/');
await page.getByRole('button', { name: /Sign out/ }).click();
await page.waitForURL(/\/signin/);
check((await owner.request.get(base + '/api/session')).status() === 401, 'signing out ends the session');

console.log(steps.map((s) => '✓ ' + s).join('\n'));
await browser.close();
