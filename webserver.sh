#!/bin/bash

trap "kill 0" EXIT

socat \
  -v -d -d \
  TCP-LISTEN:"${1:-8001}",crlf,reuseaddr,fork \
  SYSTEM:"
  echo HTTP/1.1 200 OK;
  echo Content-Type\: text/plain;
  echo;
  echo \"Server: \$SOCAT_SOCKADDR:\$SOCAT_SOCKPORT\";
  echo \"Client: \$SOCAT_PEERADDR:\$SOCAT_PEERPORT\";
  "
