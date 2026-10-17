#!/usr/bin/env python3
"""Compare exact user-level p99 distributions without pooling packets or hiding schedule delay."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path


def load(path, histogram_bounds=False):
    with path.open() as stream:
        rows = list(csv.DictReader(stream))
    if not rows:
        raise ValueError(f'{path}: no user samples')
    ids = set()
    values = {'send_to_receive': [], 'intended_send_to_receive': []}
    samples = []
    maxima = {key: 0.0 for key in values}
    actual_column = 'p99_upper_ms' if histogram_bounds else 'p99_ms'
    intended_column = 'schedule_p99_upper_ms' if histogram_bounds else 'schedule_p99_ms'
    for row in rows:
        identity = row['id']
        if not identity or identity in ids:
            raise ValueError(f'{path}: empty or duplicate user ID')
        ids.add(identity)
        count = int(row['samples'])
        if count <= 0:
            raise ValueError(f'{path}: nonpositive sample count')
        samples.append(count)
        for key, column, maximum in [('send_to_receive', actual_column, 'max_ms'),
                                     ('intended_send_to_receive', intended_column, 'schedule_max_ms')]:
            p99, top = float(row[column]), float(row[maximum])
            if not all(math.isfinite(v) and v >= 0 for v in [p99, top]) or (not histogram_bounds and p99 > top):
                raise ValueError(f'{path}: invalid latency values')
            values[key].append(p99)
            maxima[key] = max(maxima[key], top)
    return {key: sorted(v) for key, v in values.items()}, samples, maxima


def nearest(values, percent):
    """Nearest rank, consistent with the capacity client's per-user statistics."""
    return values[(len(values) * percent + 99) // 100 - 1]


def summarize(values, samples, maxima, thresholds, histogram_bounds=False):
    return {'users': len(samples), 'total_packet_samples': sum(samples),
            'samples_per_user': {'minimum': min(samples), 'maximum': max(samples)},
            'metrics': {key: {
                ('user_p99_upper_ms' if histogram_bounds else 'user_p99_ms'): {'minimum': v[0], 'median': nearest(v, 50),
                                'p95': nearest(v, 95), 'p99': nearest(v, 99), 'maximum': v[-1]},
                'worst_packet_ms': maxima[key],
                ('users_with_p99_upper_bound_at_or_below_ms_percent' if histogram_bounds else 'users_with_p99_at_or_below_ms_percent'): {
                    str(limit): 100 * sum(x <= limit for x in v) / len(v) for limit in thresholds}}
                for key, v in values.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', action='append', required=True, metavar='LABEL=CSV')
    parser.add_argument('--output', type=Path, required=True, help='Output stem for PNG, PDF and JSON')
    parser.add_argument('--subtitle', default='')
    parser.add_argument('--histogram-bounds', action='store_true',
                        help='Read bounded histogram columns; all cases must use that format')
    parser.add_argument('--threshold-ms', type=float, action='append', default=[],
                        help='Optional user-p99 threshold to summarize; repeat as needed')
    args = parser.parse_args()
    if any(not math.isfinite(v) or v <= 0 for v in args.threshold_ms):
        parser.error('thresholds must be positive finite milliseconds')
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(1, 2, figsize=(13, 5.5), layout='constrained')
    summary = {'scope': 'CDFs of user-level p99s, not pooled packet percentiles. Intended-send latency includes generator backlog. Finite workload observations, not latency bounds.',
               'percentile_method': 'nearest rank', 'cases': {}}
    summary['histogram_bounds'] = args.histogram_bounds
    if args.histogram_bounds:
        summary['scope'] = 'CDFs of user-level histogram p99 upper bounds, not exact percentiles. Threshold results describe bounds only. Maxima are exact observed durations. Intended-send includes generator backlog.'
        summary['percentile_method'] = 'nearest rank of reported histogram upper bounds'
    for case in args.case:
        label, separator, filename = case.partition('=')
        if not separator or not label or label in summary['cases']:
            parser.error('each case needs a unique nonempty LABEL=CSV')
        path = Path(filename)
        try:
            values, samples, maxima = load(path, args.histogram_bounds)
        except (OSError, ValueError, KeyError) as error:
            parser.error(str(error))
        record = summarize(values, samples, maxima, args.threshold_ms, args.histogram_bounds)
        record.update({'csv': str(path), 'csv_sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
        summary['cases'][label] = record
        for ax, key in zip(axes, values):
            ordered = values[key]
            fractions = [100 * (i + 1) / len(ordered) for i in range(len(ordered))]
            if args.histogram_bounds:
                ax.step(ordered, fractions, where='post', label=label)
            else:
                ax.plot(ordered, fractions, label=label)
    for ax, title in zip(axes, ['From actual send', 'From intended send, including backlog']):
        ax.set(xscale='symlog', xlim=(0, None), ylim=(0, 100), title=title,
               xlabel=("Each user's p99 upper bound (ms; linear below 1 ms)" if args.histogram_bounds else "Each user's p99 latency (ms; linear below 1 ms)"),
               ylabel='Users at or below latency (%)')
        ax.set_xscale('symlog', linthresh=1)
        ax.grid(alpha=.2)
        ax.legend(fontsize=8)
    fig.suptitle(args.subtitle or 'Per-user latency comparison')
    for suffix in ['.png', '.pdf']:
        fig.savefig(args.output.with_suffix(suffix), dpi=180)
    args.output.with_suffix('.json').write_text(json.dumps(summary, indent=2) + '\n')


if __name__ == '__main__':
    main()
