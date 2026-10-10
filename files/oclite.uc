#!/usr/bin/ucode
// ubus object `oclite` (rpcd ucode plugin): used by the LuCI page and by scripts, e.g.
//   ubus call oclite status
//   ubus call oclite update '{"id":"R..."}'
// over HTTP: POST /ubus with a session from `session.login` (see README).

'use strict';

import { access, popen, readfile } from 'fs';

const BIN = '/usr/bin/openwrt-clash-lite';
const SERVICE = '/etc/init.d/openwrt-clash-lite';
const RUN_DIR = '/tmp/openwrt-clash-lite';
const CORE_LOG = RUN_DIR + '/core-update.log';

function quote(s) {
	return "'" + replace(s, "'", "'\\''") + "'";
}

// (exit code, combined output) of a shell command
function run(cmd) {
	const p = popen(cmd + ' 2>&1');
	if (!p)
		return [ -1, 'cannot run ' + cmd ];
	const out = p.read('all');
	return [ p.close(), trim(out ?? '') ];
}

function result(cmd) {
	const r = run(cmd);
	return { ok: r[0] == 0, message: r[1] };
}

function cli_json(args) {
	const r = run(BIN + ' ' + args);
	return r[0] == 0 ? json(r[1]) : { error: r[1] };
}

function tail(text, lines) {
	const all = split(trim(text ?? ''), '\n');
	return join('\n', slice(all, -lines));
}

function service_running() {
	const r = run(`ubus call service list '{"name":"openwrt-clash-lite"}'`);
	return index(r[1], '"running": true') >= 0;
}

const methods = {
	status: {
		call: function() {
			const s = cli_json('status --json');
			s.service = service_running();
			s.enabled = access('/etc/rc.d/S99openwrt-clash-lite') == true;
			s.openclash = run('pidof clash clash_meta')[0] == 0 && !s.service;
			s.core_updating = access(CORE_LOG) == true && index(readfile(CORE_LOG), 'exit=') < 0;
			return s;
		}
	},
	list: {
		call: function() {
			const l = cli_json('list --json');
			return { subscriptions: type(l) == 'array' ? l : [], error: l?.error };
		}
	},
	add: {
		args: { url: 'url' },
		call: function(req) {
			const url = req.args?.url;
			if (!url || !match(url, /^https?:\/\//))
				return { ok: false, message: 'url must start with http:// or https://' };
			return result(BIN + ' add ' + quote(url));
		}
	},
	update: {
		args: { id: 'id' },
		call: function(req) {
			return result(BIN + ' update' + (req.args?.id ? ' ' + quote(req.args.id) : ''));
		}
	},
	use: {
		args: { id: 'id' },
		call: function(req) {
			return result(BIN + ' use ' + quote(req.args?.id ?? ''));
		}
	},
	remove: {
		args: { id: 'id' },
		call: function(req) {
			return result(BIN + ' remove ' + quote(req.args?.id ?? ''));
		}
	},
	service: {
		args: { action: 'action' },
		call: function(req) {
			const action = req.args?.action;
			if (!(action in [ 'start', 'stop', 'restart', 'enable', 'disable' ]))
				return { ok: false, message: 'action must be start, stop, restart, enable or disable' };
			return result(SERVICE + ' ' + action);
		}
	},
	update_core: {
		call: function() {
			system('mkdir -p ' + RUN_DIR);
			system(`(${BIN} update-core > ${CORE_LOG} 2>&1; echo "exit=$?" >> ${CORE_LOG}) >/dev/null 2>&1 &`);
			return { ok: true, message: 'started; follow it with log {"name":"core"}' };
		}
	},
	log: {
		args: { name: 'name', lines: 200 },
		call: function(req) {
			const lines = req.args?.lines ?? 200;
			switch (req.args?.name) {
			case 'mihomo':
				return { log: tail(readfile(RUN_DIR + '/mihomo.log'), lines) };
			case 'core':
				return { log: tail(readfile(CORE_LOG), lines) };
			default:
				return { log: tail(run('logread -e openwrt-clash-lite')[1], lines) };
			}
		}
	}
};

return { 'oclite': methods };
