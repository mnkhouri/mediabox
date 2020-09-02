from transmission_rpc import Client
import requests

MAX_SEEDERS = 5

print (f'Pausing all with >{MAX_SEEDERS} seeders or no leechers')
c = Client(host='IP_GOES_HERE')
torrents = c.get_torrents()
pausedCount = 0

for t in torrents:
    if t.status == 'seeding':
        name = t.name

        seeders = max((stats['seederCount'] for stats in t.trackerStats), default=0)
        if seeders > MAX_SEEDERS:
            print(f'[{seeders:03}] {name}')
            t.stop()
            pausedCount = pausedCount + 1

        has_peers = any(stats['leecherCount'] > 0 for stats in t.trackerStats)
        if not has_peers:
            print(f'[nop] {name}')
            t.stop()
            pausedCount = pausedCount + 1

print(f'Paused {pausedCount}')
