"""Optional verified fanout pressure beside the stable connected population."""
import json


EXPECTED_ERRORS = {"war_slow_consumers_total", "war_aborted_writes_total",
                   "war_close_failures_total", "war_transport_disconnects_total"}


def finish(process):
    stdout, stderr = process.communicate(timeout=10)
    assert process.returncode == 0, stderr
    events = [json.loads(row) for row in stdout.splitlines()]
    assert [event["event"] for event in events] == ["ready", "traffic_finished", "result"]
    result = events[-1]
    assert result["mode"] == "fanout" and result["unread_clients"] == 8
    assert result["healthy_readers"] == 1 and result["verified_deliveries"] > 0
    assert result["disconnect_notifications"] == 8 and result["teardown_capacity_closes"] == 0
    assert len(result["unread_observations"]) == 8
    assert all(item["kind"] in ("close", "eof", "transport_error")
               and (item["kind"] != "close" or item["code"] == 1013)
               for item in result["unread_observations"])
    return {"events": events, "stderr": stderr, "exit_code": process.returncode}


def adapt_checks(expected, observed, normal_counts, pressure, event):
    """Keep exact input/reuse/lifecycle checks; bound intentional discarded output.

    The existing stress client verifies all healthy and buffered unread frames.
    It does not count its publisher's consumed departure notifications or welcome
    byte lengths. Those contributions are bounded explicitly rather than falsely
    attributed to the independently counted stable client.
    """
    result = pressure["events"][-1]
    expected["war_accepted_total"] += 10
    expected["war_joined_total"] += 10
    expected["war_disconnected_total"] += 10
    expected["war_incoming_messages_total"] += result["sent_messages"]
    expected["war_incoming_bytes_total"] += result["sent_encoded_bytes"]
    expected["war_routing_reused_bytes_total"] += result["sent_value_bytes"]
    expected["war_slow_consumers_total"] = 8
    for name in EXPECTED_ERRORS - {"war_slow_consumers_total"}:
        expected.pop(name, None)
        assert 0 <= observed[name] <= 8, (name, observed[name])
    # At most nine fanout recipients and ten members' departure notifications.
    delivered = observed["war_deliveries_total"] - normal_counts["deliveries"]
    delivered_bytes = observed["war_delivered_bytes_total"] - normal_counts["delivered_bytes"]
    assert result["verified_deliveries"] + 10 <= delivered <= result["sent_messages"] * 9 + 110
    assert result["verified_deliveries"] * 1027 <= delivered_bytes <= result["sent_encoded_bytes"] * 9 + 2200
    expected.pop("war_deliveries_total")
    expected.pop("war_delivered_bytes_total")
    event["pressure_output_bounds"] = {
        "additional_deliveries": delivered, "additional_delivered_bytes": delivered_bytes,
        "reason": "Intentional unread-recipient cancellation; sidecar validates healthy/buffered full frames. Publisher notifications/welcome sizes are not independently counted by that client."}


def command(path, url, cpus):
    result = [str(path.resolve()), "--url", url, "--mode", "fanout", "--clients", "10",
              "--unread-clients", "8", "--duration-secs", "10", "--payload-bytes", "1024", "--window", "16"]
    return ["taskset", "-c", cpus] + result if cpus else result
