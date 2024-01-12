#!/bin/sh
echo "Did you start wireshark?"
# echo "What is the private LTE interface name?"
# read private_lte_if
# echo $private_lte_if

sudo ls > /dev/null

# Get default interfaces and identify active interface

gws=$(route | grep default | awk '{print $8}')
act_gw=$(route | grep default | awk '{print $8}' | awk 'NR==1')

echo "Gateways:\n$gws"

echo "Active if:\n$act_gw"

# Start with this default interface down

sudo ip link set $act_gw down

# Delay packets so the QUIC request can be interrupted

# sudo tc qdisc add dev $act_gw root netem delay 100ms

# Start QUIC client request

cargo run --bin quiche-client -- https://128.173.236.251:4433 --no-verify > /dev/null &


# Interrupt request by bringing down active interface
sleep 1

# date +%H:%M:%S:%N
sudo ip link set $act_gw up

# echo "link down"

# sleep 3

# Bring interface back up and remove packet delay
# sudo tc qdisc del dev $act_gw root netem

