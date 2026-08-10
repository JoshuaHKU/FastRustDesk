import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';

/// LOCAL FORK: devices registered with the self-hosted ID server.
///
/// Replaces the upstream "Accessible devices" panel, which is a RustDesk Server
/// Pro feature: it fetches `/api/device-group/accessible` from the API server on
/// port 21114 and requires an account login. The open-source hbbs has no such
/// endpoint, so upstream that tab can only ever show a login button.
///
/// This asks hbbs itself over the rendezvous protocol instead — no account, no
/// HTTP API. The server requires the shared key, so only clients already
/// configured for this server can enumerate it.
class ServerPeer {
  final String id;
  final bool online;
  final String ip;

  /// Milliseconds since the device last registered, or -1 when the server has
  /// not seen it since it started (known to exist, liveness unknown).
  final int lastRegMs;
  final String hostname;
  final String platform;

  /// The name the user gave this device, or empty.
  ///
  /// **Not from the server.** An alias is a local, per-operator label -- the
  /// same machine may sensibly be "客厅 Mac" here and "build box" elsewhere --
  /// so the ID server neither stores nor needs it. Filled in by
  /// [ServerPeersModel._resolveAlias] after the roster arrives.
  String alias;

  ServerPeer({
    required this.id,
    required this.online,
    required this.ip,
    required this.lastRegMs,
    required this.hostname,
    required this.platform,
    this.alias = '',
  });

  factory ServerPeer.fromJson(Map<String, dynamic> j) => ServerPeer(
        id: j['id'] as String? ?? '',
        online: j['online'] as bool? ?? false,
        ip: j['ip'] as String? ?? '',
        lastRegMs: (j['last_reg_ms'] as num?)?.toInt() ?? -1,
        hostname: j['hostname'] as String? ?? '',
        platform: j['platform'] as String? ?? '',
      );

  /// What to show as the primary label.
  ///
  /// A name the user chose beats one the machine reports: renaming a device in
  /// 「最近访问过」 is an explicit statement about how they want to see it, and
  /// it would be odd for the same device to answer to two different names in two
  /// tabs. The hostname is the fallback, and the id the fallback of last resort
  /// (a device registered by a client too old to report a hostname).
  String get displayName {
    if (alias.isNotEmpty) return alias;
    if (hostname.isNotEmpty) return hostname;
    return id;
  }
}

class ServerPeersModel with ChangeNotifier {
  WeakReference<FFI> parent;

  ServerPeersModel(this.parent);

  List<ServerPeer> _peers = [];
  bool _loading = false;
  String _error = '';

  List<ServerPeer> get peers => _peers;
  bool get loading => _loading;

  /// Non-empty when the last refresh failed. Distinguishing this from an empty
  /// list matters: "the server refused us" and "no other devices yet" look the
  /// same in the UI otherwise.
  String get error => _error;

  Future<void> pull({bool quiet = false}) async {
    if (_loading) return;
    if (!quiet) {
      _loading = true;
      notifyListeners();
    }
    try {
      final raw = await bind.mainGetCommon(key: 'server-peers');
      final decoded = jsonDecode(raw);
      if (decoded is Map && decoded['error'] != null) {
        _error = decoded['error'].toString();
        _peers = [];
      } else if (decoded is Map && decoded['peers'] is List) {
        _peers = (decoded['peers'] as List)
            .whereType<Map<String, dynamic>>()
            .map(ServerPeer.fromJson)
            .toList();
        // Resolve aliases **before** sorting -- the sort key is displayName, so
        // filling them in afterwards would leave the list ordered by names it no
        // longer shows.
        for (final p in _peers) {
          p.alias = await _resolveAlias(p.id);
        }
        // Online first, then by name, so the machines you can actually reach
        // are not buried under stale entries.
        _peers.sort((a, b) {
          if (a.online != b.online) return a.online ? -1 : 1;
          return a.displayName.compareTo(b.displayName);
        });
        _error = '';
      } else {
        _error = 'unexpected response';
        _peers = [];
      }
    } catch (e) {
      _error = e.toString();
      _peers = [];
    } finally {
      _loading = false;
      notifyListeners();
    }
    await _syncToAddressBook();
  }

  /// LOCAL FORK: put group devices into the address book if they are missing.
  ///
  /// The two lists answer different questions -- 「可访问的设备」 is "what does
  /// the server say exists", 地址簿 is "what have I saved" -- and a device you can
  /// reach but have never saved falls between them. This closes that gap on
  /// every refresh, including the one at startup.
  ///
  /// Deliberately quiet and best-effort:
  ///
  ///   * **Requires a login.** With no account there is no address book to add
  ///     to, and every add would fail.
  ///   * **Skips this machine.** Its own id is in the roster, and an entry for
  ///     yourself in your own address book is just noise.
  ///   * **Never surfaces an error.** This runs unasked on every refresh;
  ///     interrupting the user because one add failed would be worse than the
  ///     device simply not appearing. Note `addIdToCurrent` *returns* a message
  ///     for "already exists" rather than throwing -- that is the common case
  ///     and not a failure, which is why the return value is ignored.
  Future<void> _syncToAddressBook() async {
    final ffi = parent.target;
    if (ffi == null) return;
    if (!ffi.userModel.isLogin) return;
    if (ffi.abModel.currentName.value.isEmpty) return;
    String myId = '';
    try {
      myId = await bind.mainGetMyId();
    } catch (_) {
      // Not knowing our own id would mean adding ourselves; skip the whole pass
      // rather than pollute the address book.
      return;
    }
    for (final p in _peers) {
      if (p.id.isEmpty || p.id == myId) continue;
      if (ffi.abModel.find(p.id) != null) continue;
      try {
        // Carry a name across too: a device added silently and shown as nine
        // digits is not much use. Falls back to the hostname the server reports.
        await ffi.abModel.addIdToCurrent(
          p.id,
          p.alias.isNotEmpty ? p.alias : p.hostname,
          '',
          [],
          '',
        );
      } catch (_) {
        // Keep going -- one device failing must not stop the rest.
      }
    }
  }

  /// The user's name for a device, or '' if they never gave it one.
  ///
  /// Renaming in 「最近访问过」 writes both the local peer config
  /// (`mainSetPeerAlias`) and the address book (`abModel.changeAlias`), so the
  /// local config is checked first: it is the direct target of that rename and
  /// exists even with no account. The address book is the fallback, which picks
  /// up a device renamed on *another* machine and synced here.
  Future<String> _resolveAlias(String id) async {
    try {
      final local = await bind.mainGetPeerOption(id: id, key: 'alias');
      if (local.isNotEmpty) return local;
    } catch (_) {
      // No local config for this peer is normal: this list shows every device
      // on the server, including ones never connected to from here.
    }
    try {
      return parent.target?.abModel.find(id)?.alias ?? '';
    } catch (_) {
      return '';
    }
  }
}
