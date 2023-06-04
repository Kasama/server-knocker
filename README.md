Server Knocker
==============

Server knocker is an application that can listen to a port and proxy packets to another application.

The proxy will also wait for a idle timeout. If it doesn't get any new packets in this time, it will kill the child application.

Then, if it receives a new packet, it will spawn the child application again so it can continue proxying packets to it.

Motivation
==========

The goal of server knocker is to save resources when running some expensive application that is not used often. For example, a game server to play with friends every once in a while without wasting resources in the server.

Think a minecraft server that can automatically shut down during the night, and spin up automatically when someone tries to join in.

Usage
=====

Use `server-knocker --help` for detailed up-to-date information.
