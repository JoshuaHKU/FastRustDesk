import 'package:flutter/material.dart';
import 'package:flutter_hbb/models/server_peers_model.dart';
import 'package:provider/provider.dart';

import '../../common.dart';

/// LOCAL FORK: the "Accessible devices" panel, backed by the self-hosted ID
/// server instead of a RustDesk Server Pro account.
///
/// Upstream this panel is `MyGroup`, which shows a Login button until the user
/// signs in to the Pro API on port 21114. The open-source hbbs has no such API,
/// so that panel is permanently a dead end here. This lists what hbbs itself
/// knows about; tapping a row connects.
class ServerDevices extends StatefulWidget {
  final EdgeInsets? menuPadding;
  const ServerDevices({Key? key, this.menuPadding}) : super(key: key);

  @override
  State<StatefulWidget> createState() => _ServerDevicesState();
}

class _ServerDevicesState extends State<ServerDevices> {
  @override
  void initState() {
    super.initState();
    // Pull once when the panel first appears rather than polling: the list only
    // changes when a machine is added or goes offline, and a background poll
    // against the ID server would be pure noise.
    WidgetsBinding.instance
        .addPostFrameCallback((_) => gFFI.serverPeersModel.pull());
  }

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProvider.value(
      value: gFFI.serverPeersModel,
      child: Consumer<ServerPeersModel>(
        builder: (context, model, child) {
          if (model.loading && model.peers.isEmpty) {
            return const Center(child: CircularProgressIndicator());
          }
          return Column(
            children: [
              _buildHeader(model),
              Expanded(child: _buildBody(model)),
            ],
          );
        },
      ),
    );
  }

  Widget _buildHeader(ServerPeersModel model) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      child: Row(
        children: [
          Expanded(
            child: Text(
              translate('Devices on ID server'),
              style: const TextStyle(fontWeight: FontWeight.bold),
            ),
          ),
          IconButton(
            tooltip: translate('Refresh'),
            icon: const Icon(Icons.refresh),
            onPressed: model.loading ? null : () => model.pull(),
          ),
        ],
      ),
    );
  }

  Widget _buildBody(ServerPeersModel model) {
    if (model.error.isNotEmpty) {
      // Show the server's own words. The common causes are a key mismatch and an
      // ID server that has not been upgraded to answer this request, and the
      // message distinguishes them.
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.cloud_off, size: 40),
              const SizedBox(height: 12),
              Text(translate('Failed to list devices'),
                  style: const TextStyle(fontWeight: FontWeight.bold)),
              const SizedBox(height: 6),
              Text(model.error, textAlign: TextAlign.center),
              const SizedBox(height: 12),
              ElevatedButton(
                onPressed: () => model.pull(),
                child: Text(translate('Retry')),
              ),
            ],
          ),
        ),
      );
    }
    if (model.peers.isEmpty) {
      return Center(child: Text(translate('No other devices on this server')));
    }
    return ListView.separated(
      itemCount: model.peers.length,
      separatorBuilder: (_, __) => const Divider(height: 1),
      itemBuilder: (context, i) => _buildRow(model.peers[i]),
    );
  }

  Widget _buildRow(ServerPeer p) {
    return ListTile(
      leading: Stack(
        alignment: Alignment.bottomRight,
        children: [
          Icon(_platformIcon(p.platform), size: 30),
          Container(
            width: 10,
            height: 10,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: p.online ? Colors.green : Colors.grey,
            ),
          ),
        ],
      ),
      title: Text(p.displayName),
      subtitle: Text(_subtitle(p)),
      trailing: const Icon(Icons.chevron_right),
      onTap: () => connect(context, p.id),
    );
  }

  String _subtitle(ServerPeer p) {
    final parts = <String>[p.id];
    // When a user-given name is displayed above, show the hostname here --
    // otherwise renaming a device costs you the only clue as to which physical
    // machine it is.
    if (p.alias.isNotEmpty && p.hostname.isNotEmpty) parts.add(p.hostname);
    if (p.hostname.isNotEmpty && p.ip.isNotEmpty) parts.add(p.ip);
    if (p.online) {
      parts.add(translate('Online'));
    } else if (p.lastRegMs >= 0) {
      parts.add('${translate('Last seen')}: ${_ago(p.lastRegMs)}');
    } else {
      parts.add(translate('Offline'));
    }
    return parts.join('  ·  ');
  }

  String _ago(int ms) {
    final s = ms ~/ 1000;
    if (s < 60) return '${s}s';
    final m = s ~/ 60;
    if (m < 60) return '${m}m';
    final h = m ~/ 60;
    if (h < 24) return '${h}h';
    return '${h ~/ 24}d';
  }

  IconData _platformIcon(String platform) {
    switch (platform) {
      case 'Windows':
        return Icons.desktop_windows;
      case 'Mac OS':
        return Icons.laptop_mac;
      case 'Linux':
        return Icons.computer;
      case 'Android':
        return Icons.phone_android;
      default:
        return Icons.devices_other;
    }
  }
}
