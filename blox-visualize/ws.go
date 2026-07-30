// A minimal RFC 6455 server — just enough for a local dashboard: the
// handshake, text frames out, control frames in, no extensions, no
// permessage-deflate. Keeping it dependency-free means the visualizer
// builds with an empty module cache, same as blox-sim.
package main

import (
	"bufio"
	"crypto/sha1"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
)

const (
	opContinuation = 0x0
	opText         = 0x1
	opBinary       = 0x2
	opClose        = 0x8
	opPing         = 0x9
	opPong         = 0xa
)

type wsFrame struct {
	op   byte
	data []byte
}

type wsConn struct {
	conn net.Conn
	r    *bufio.Reader
	send chan wsFrame
	done chan struct{}
}

const wsGUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

// wsMaxFrame bounds client frames. The UI sends nothing but pongs; anything
// bigger is a bug or an attack, and either way not worth buffering.
const wsMaxFrame = 1 << 20

func upgrade(w http.ResponseWriter, r *http.Request) (*wsConn, error) {
	if !headerContains(r.Header, "Connection", "upgrade") ||
		!headerContains(r.Header, "Upgrade", "websocket") {
		return nil, errors.New("not a websocket upgrade")
	}
	key := r.Header.Get("Sec-WebSocket-Key")
	if key == "" {
		return nil, errors.New("missing Sec-WebSocket-Key")
	}
	hj, ok := w.(http.Hijacker)
	if !ok {
		return nil, errors.New("hijacking unsupported")
	}
	conn, rw, err := hj.Hijack()
	if err != nil {
		return nil, err
	}
	sum := sha1.Sum([]byte(key + wsGUID))
	if _, err := fmt.Fprintf(rw, "HTTP/1.1 101 Switching Protocols\r\n"+
		"Upgrade: websocket\r\nConnection: Upgrade\r\n"+
		"Sec-WebSocket-Accept: %s\r\n\r\n",
		base64.StdEncoding.EncodeToString(sum[:])); err != nil {
		_ = conn.Close()
		return nil, err
	}
	if err := rw.Flush(); err != nil {
		_ = conn.Close()
		return nil, err
	}
	return &wsConn{
		conn: conn,
		r:    rw.Reader,
		send: make(chan wsFrame, 256),
		done: make(chan struct{}),
	}, nil
}

func headerContains(h http.Header, name, val string) bool {
	for _, s := range h[http.CanonicalHeaderKey(name)] {
		for _, part := range strings.Split(s, ",") {
			if strings.EqualFold(strings.TrimSpace(part), val) {
				return true
			}
		}
	}
	return false
}

// writeLoop is the only writer to the socket.
func (c *wsConn) writeLoop() {
	defer c.close()
	w := bufio.NewWriterSize(c.conn, 1<<14)
	for {
		select {
		case <-c.done:
			return
		case frame := <-c.send:
			if err := writeFrame(w, frame.op, frame.data); err != nil {
				return
			}
			// Coalesce whatever is already queued into one flush — the same
			// trick as blox-server's own writer.
			n := len(c.send)
			for i := 0; i < n; i++ {
				select {
				case frame = <-c.send:
					if err := writeFrame(w, frame.op, frame.data); err != nil {
						return
					}
				default:
				}
			}
			if err := w.Flush(); err != nil {
				return
			}
		}
	}
}

// readLoop consumes client frames, answering pings, honouring close, and
// dispatching reassembled text messages to onMessage. It returns when the
// connection dies; onClose fires exactly once per client.
func (c *wsConn) readLoop(onMessage func([]byte), onClose func()) {
	defer onClose()
	defer c.close()
	var frag []byte
	for {
		fin, op, payload, err := c.readFrame()
		if err != nil {
			return
		}
		switch op {
		case opClose:
			c.reply(opClose, nil)
			return
		case opPing:
			c.reply(opPong, payload)
		case opText, opContinuation:
			frag = append(frag, payload...)
			if fin {
				onMessage(frag)
				frag = frag[:0]
			}
		case opBinary:
			// The UI never speaks binary; drain and ignore.
		}
	}
}

func (c *wsConn) reply(op byte, data []byte) {
	select {
	case <-c.done:
		return
	default:
	}
	select {
	case c.send <- wsFrame{op: op, data: data}:
	default:
	}
}

func (c *wsConn) close() {
	select {
	case <-c.done:
	default:
		close(c.done)
		_ = c.conn.Close()
	}
}

func writeFrame(w *bufio.Writer, op byte, payload []byte) error {
	if err := w.WriteByte(0x80 | op); err != nil {
		return err
	}
	n := len(payload)
	switch {
	case n < 126:
		if err := w.WriteByte(byte(n)); err != nil {
			return err
		}
	case n <= 0xffff:
		if err := w.WriteByte(126); err != nil {
			return err
		}
		var b [2]byte
		binary.BigEndian.PutUint16(b[:], uint16(n))
		if _, err := w.Write(b[:]); err != nil {
			return err
		}
	default:
		if err := w.WriteByte(127); err != nil {
			return err
		}
		var b [8]byte
		binary.BigEndian.PutUint64(b[:], uint64(n))
		if _, err := w.Write(b[:]); err != nil {
			return err
		}
	}
	_, err := w.Write(payload)
	return err
}

func (c *wsConn) readFrame() (fin bool, op byte, payload []byte, err error) {
	var hdr [2]byte
	if _, err = io.ReadFull(c.r, hdr[:]); err != nil {
		return
	}
	fin = hdr[0]&0x80 != 0
	op = hdr[0] & 0x0f
	masked := hdr[1]&0x80 != 0
	ln := uint64(hdr[1] & 0x7f)
	switch ln {
	case 126:
		var b [2]byte
		if _, err = io.ReadFull(c.r, b[:]); err != nil {
			return
		}
		ln = uint64(binary.BigEndian.Uint16(b[:]))
	case 127:
		var b [8]byte
		if _, err = io.ReadFull(c.r, b[:]); err != nil {
			return
		}
		ln = binary.BigEndian.Uint64(b[:])
	}
	if ln > wsMaxFrame {
		err = errors.New("frame too large")
		return
	}
	var mask [4]byte
	if masked {
		if _, err = io.ReadFull(c.r, mask[:]); err != nil {
			return
		}
	}
	payload = make([]byte, ln)
	if _, err = io.ReadFull(c.r, payload); err != nil {
		return
	}
	if masked {
		for i := range payload {
			payload[i] ^= mask[i&3]
		}
	}
	return
}
