'use strict';
'require view';
'require form';
'require rpc';
'require poll';
'require ui';
'require uci';

const oclite = (method, params) => rpc.declare({
	object: 'oclite', method: method, params: params || [], expect: { '': {} }
});
const callStatus = oclite('status');
const callList = oclite('list');
const callAdd = oclite('add', [ 'url' ]);
const callEdit = oclite('edit', [ 'id', 'name', 'url' ]);
const callUpdate = oclite('update', [ 'id' ]);
const callUse = oclite('use', [ 'id' ]);
const callRemove = oclite('remove', [ 'id' ]);
const callService = oclite('service', [ 'action' ]);
const callUpdateCore = oclite('update_core');
const callLog = oclite('log', [ 'name', 'lines' ]);
const callRcInit = rpc.declare({
	object: 'rc', method: 'init', params: [ 'name', 'action' ], expect: { '': {} }
});

let status = {};

function failed(res) {
	if (res && res.ok === false) {
		ui.addNotification(null, E('p', res.message || _('操作失败')), 'danger');
		return true;
	}
	return false;
}

function stateText(s) {
	if (!s.service)
		return [ 'red', _('已停止') ];
	if (!s.running)
		return [ 'orange', _('启动中…（长时间停在这里请看下方日志）') ];
	if (!s.tun)
		return [ 'orange', _('加载规则中…（局域网暂时直连）') ];
	return [ 'green', _('运行中，已接管') ];
}

function yacdUrl(s) {
	const host = window.location.hostname;
	return 'http://%s:%d/ui/?hostname=%s&port=%d&secret=%s'.format(
		host, s.port, encodeURIComponent(host), s.port, encodeURIComponent(s.secret));
}

/* restart only when running, so switching or updating applies without starting a stopped service */
function applyIfRunning() {
	return status.service ? callService('restart').then(refreshStatus) : Promise.resolve();
}

function refreshStatus() {
	return L.resolveDefault(callStatus(), {}).then(function(s) {
		status = s;
		const [ color, text ] = stateText(s);
		const set = (id, value) => { const el = document.getElementById(id); if (el) el.textContent = value; };
		const light = document.getElementById('oclite-state');
		if (light) light.style.color = color;
		set('oclite-state', text);
		set('oclite-version', s.version || '-');
		set('oclite-memory', s.memory_kb ? '%.1f MB'.format(s.memory_kb / 1024) : '-');
		set('oclite-active', s.active ? s.active.name : _('无（请先添加订阅）'));
		set('oclite-core', s.core || _('未找到 mihomo，请点「更新内核」'));
		set('oclite-boot', s.enabled ? _('关闭开机自启') : _('开启开机自启'));
		const warn = document.getElementById('oclite-openclash');
		if (warn) warn.style.display = s.openclash ? '' : 'none';
	});
}

function waitCoreUpdate() {
	return new Promise(function(resolve) {
		const tick = function() {
			callLog('core', 20).then(function(r) {
				if ((r.log || '').indexOf('exit=') < 0)
					return window.setTimeout(tick, 2000);
				resolve(r.log);
			});
		};
		tick();
	});
}

function updateCore() {
	ui.showModal(_('更新 mihomo 内核'), [ E('p', { 'class': 'spinning' }, _('正在从 GitHub 下载最新版本，约需 10–60 秒…')) ]);
	return callUpdateCore().then(waitCoreUpdate).then(function(log) {
		ui.hideModal();
		const ok = log.indexOf('exit=0') >= 0;
		const current = log.indexOf('already up to date') >= 0;
		const text = current ? _('已是最新版本，无需更新：') + log.replace(/^already up to date: /, '') : log;
		ui.addNotification(null, E('pre', text.replace(/\nexit=\d+$/, '')), ok ? 'info' : 'danger');
		return ok && !current ? applyIfRunning() : null;
	});
}

function row(label, id, extra) {
	return E('tr', { 'class': 'tr' }, [
		E('td', { 'class': 'td left', 'width': '25%' }, label),
		E('td', { 'class': 'td left' }, [ E('span', { 'id': id }, '-'), extra || '' ])
	]);
}

function button(label, style, fn) {
	return E('button', { 'class': 'cbi-button cbi-button-' + style, 'click': ui.createHandlerFn(this, fn) }, label);
}

function service(action) {
	return callService(action).then(function(res) {
		if (!failed(res))
			return refreshStatus();
	});
}

function renderStatus() {
	return E('div', { 'class': 'cbi-section' }, [
		E('h3', _('运行状态')),
		E('div', { 'id': 'oclite-openclash', 'class': 'alert-message warning', 'style': 'display:none' }, [
			_('检测到 OpenClash 正在运行，两者不能同时接管网络。'), ' ',
			button(_('停止 OpenClash'), 'negative', () => callRcInit('openclash', 'stop').then(refreshStatus))
		]),
		E('table', { 'class': 'table' }, [
			row(_('状态'), 'oclite-state'),
			row(_('mihomo 版本'), 'oclite-version'),
			row(_('内存占用'), 'oclite-memory'),
			row(_('当前订阅'), 'oclite-active'),
			row(_('内核文件'), 'oclite-core')
		]),
		E('div', { 'style': 'margin:1em 0' }, [
			button(_('启动'), 'apply', () => service('start')), ' ',
			button(_('停止'), 'reset', () => service('stop')), ' ',
			button(_('重启'), 'action', () => service('restart')), ' ',
			E('button', { 'id': 'oclite-boot', 'class': 'cbi-button cbi-button-neutral',
				'click': ui.createHandlerFn(this, () => service(status.enabled ? 'disable' : 'enable')) }, '-'), ' ',
			button(_('打开 Yacd 面板'), 'positive', function() {
				if (!status.running)
					return ui.addNotification(null, E('p', _('请先启动服务')), 'warning');
				window.open(yacdUrl(status), '_blank');
			}), ' ',
			button(_('更新内核'), 'neutral', updateCore)
		])
	]);
}

function editDialog(sub, reload) {
	const field = (value) => E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:100%', 'value': value });
	const name = field(sub.name);
	const url = field(sub.url);
	const error = E('p', { 'class': 'alert-message danger', 'style': 'display:none' });
	const save = function() {
		const newUrl = url.value.trim();
		return callEdit(sub.id, name.value, newUrl).then(function(r) {
			if (r && r.ok === false) {
				error.textContent = r.message || _('保存失败');
				error.style.display = '';
				return;
			}
			ui.hideModal();
			const restart = sub.active && newUrl !== sub.url ? applyIfRunning() : Promise.resolve();
			return restart.then(reload);
		});
	};
	ui.showModal(_('编辑订阅'), [
		E('p', _('名称')), name,
		E('p', _('订阅链接（修改后会先下载验证，下载失败则不做任何改动）')), url,
		error,
		E('div', { 'class': 'right', 'style': 'margin-top:1em' }, [
			E('button', { 'class': 'cbi-button', 'click': ui.hideModal }, _('取消')), ' ',
			button(_('保存'), 'positive', save)
		])
	]);
}

function subscriptionRow(sub, reload) {
	const when = sub.updated ? new Date(sub.updated * 1000).toLocaleString() : '-';
	const act = (fn) => () => fn().then(function(res) { if (!failed(res)) return reload(); });
	return E('tr', { 'class': 'tr' }, [
		E('td', { 'class': 'td' }, [ sub.active ? E('strong', '✔ ') : '', sub.name ]),
		E('td', { 'class': 'td' }, when),
		E('td', { 'class': 'td right' }, [
			sub.active ? '' : button(_('使用'), 'apply',
				act(() => callUse(sub.id).then((r) => failed(r) ? r : applyIfRunning().then(() => r)))), ' ',
			button(_('编辑'), 'edit', () => editDialog(sub, reload)), ' ',
			button(_('更新'), 'action',
				act(() => callUpdate(sub.id).then((r) => (!failed(r) && sub.active) ? applyIfRunning().then(() => r) : r))), ' ',
			button(_('删除'), 'negative', act(function() {
				return confirm(_('删除订阅「%s」？').format(sub.name)) ? callRemove(sub.id) : Promise.resolve({});
			}))
		])
	]);
}

function renderSubscriptions(list) {
	const body = E('tbody');
	const input = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:60%',
		'placeholder': _('粘贴订阅链接 https://…') });
	const reload = function() {
		return callList().then(function(r) { fill(r.subscriptions || []); return refreshStatus(); });
	};
	const fill = function(subs) {
		body.replaceChildren(...(subs.length ? subs.map((s) => subscriptionRow(s, reload))
			: [ E('tr', { 'class': 'tr' }, E('td', { 'class': 'td', 'colspan': 3 }, _('还没有订阅'))) ]));
	};
	fill(list);
	return E('div', { 'class': 'cbi-section' }, [
		E('h3', _('订阅')),
		E('table', { 'class': 'table' }, [
			E('thead', E('tr', { 'class': 'tr table-titles' }, [
				E('th', { 'class': 'th' }, _('名称')), E('th', { 'class': 'th' }, _('更新时间')),
				E('th', { 'class': 'th right' }, _('操作')) ])),
			body
		]),
		E('div', {}, [ input, ' ', button(_('添加订阅'), 'add', function() {
			return callAdd(input.value.trim()).then(function(res) {
				if (!failed(res)) { input.value = ''; return reload(); }
			});
		}) ])
	]);
}

function renderSettings() {
	const m = new form.Map('openwrt-clash-lite');
	const s = m.section(form.NamedSection, 'main', 'main', _('设置'),
		_('修改后点页面底部的「保存并应用」。日志级别和端口还要再点上方的「重启」才生效。'));
	let o = s.option(form.ListValue, 'log_level', _('mihomo 日志级别'));
	[ 'silent', 'error', 'warning', 'info', 'debug' ].forEach((l) => o.value(l));
	o.default = 'warning';
	o = s.option(form.Value, 'port', _('面板 / API 端口'), _('Yacd 面板和 mihomo API 使用的端口，局域网可访问，由密钥保护。'));
	o.datatype = 'port';
	o.placeholder = '19097';
	o = s.option(form.Flag, 'auto_update', _('每天自动更新订阅'),
		_('到时间重新下载当前订阅；内容有变化且服务在运行时才重启，没变化不会断网。保存即生效。'));
	o.rmempty = false;
	o = s.option(form.Value, 'update_time', _('更新时间'), _('24 小时制，例如 06:10'));
	o.placeholder = '06:10';
	o.depends('auto_update', '1');
	o.validate = (id, v) => /^([01]\d|2[0-3]):[0-5]\d$/.test(v) || _('请按 06:10 这样的格式填写');
	return m.render();
}

function renderLog() {
	const area = E('textarea', { 'class': 'cbi-input-textarea', 'readonly': true, 'wrap': 'off', 'rows': 18,
		'style': 'width:100%;font-family:monospace;font-size:12px' });
	const select = E('select', { 'class': 'cbi-input-select' }, [
		E('option', { 'value': 'service' }, _('服务日志')), E('option', { 'value': 'mihomo' }, _('mihomo 日志')) ]);
	const load = () => callLog(select.value, 200).then(function(r) {
		area.value = r.log || _('（空）');
		area.scrollTop = area.scrollHeight;
	});
	select.addEventListener('change', load);
	load();
	return E('div', { 'class': 'cbi-section' }, [
		E('h3', _('日志')), E('div', {}, [ select, ' ', button(_('刷新'), 'action', load) ]), area ]);
}

return view.extend({
	load: function() {
		return Promise.all([ L.resolveDefault(callList(), {}), uci.load('openwrt-clash-lite') ]);
	},

	render: function(data) {
		const subs = (data[0] && data[0].subscriptions) || [];
		return Promise.resolve(renderSettings()).then(function(settings) {
			const page = E('div', {}, [
				E('h2', _('Clash Lite')),
				E('div', { 'class': 'cbi-map-descr' }, _('轻量的 mihomo 透明代理（TUN 接管）。')),
				renderStatus(), renderSubscriptions(subs), settings, renderLog()
			]);
			refreshStatus();
			poll.add(refreshStatus, 5);
			return page;
		});
	}
});
