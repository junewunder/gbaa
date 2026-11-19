/* A C ENet relay between two Rogue co-op games, to prove gbaa's rusty_enet
 * speaks the wire of the ENet the Rogue Assistant links (1.3.17).
 *
 * It listens for the joining game on JOIN_PORT and connects to the hosting
 * game on HOST_PORT (127.0.0.1), and passes every packet across on the same
 * channel with the same flags. Nothing is passed on until both sides are
 * connected: what arrives before that waits in a queue, so the joiner's
 * handshake isn't lost while the relay is still reaching the host. A connect
 * to the host that times out is tried again.
 *
 * Build (ENet 1.3.17's sources, not in git):
 *   gcc -O1 -I$ENET/include -DHAS_SOCKLEN_T -o relay relay.c \
 *       $ENET/{callbacks,compress,host,list,packet,peer,protocol,unix}.c
 * Run: relay HOST_PORT JOIN_PORT SECONDS
 */
#include <enet/enet.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#define CHANNELS 4
#define QUEUE_MAX 4096

typedef struct {
    ENetPacket *packet;
    enet_uint8 channel;
} Queued;

/* Packets waiting for the other side, one queue per direction. */
static Queued to_host[QUEUE_MAX], to_join[QUEUE_MAX];
static int to_host_n, to_join_n;
/* Counts passed on, per direction and channel. */
static long passed_to_host[CHANNELS], passed_to_join[CHANNELS];

static ENetPeer *host_peer, *join_peer;
static int host_up, join_up;

static void pass(ENetPeer *to, ENetPacket *p, enet_uint8 ch, long *count) {
    /* A fresh packet with the same flags: the received one is the
     * receiving host's to free. */
    ENetPacket *copy = enet_packet_create(p->data, p->dataLength,
                                          p->flags & (ENET_PACKET_FLAG_RELIABLE |
                                                      ENET_PACKET_FLAG_UNSEQUENCED));
    if (enet_peer_send(to, ch, copy) < 0) {
        enet_packet_destroy(copy);
        fprintf(stderr, "relay: send failed on channel %u\n", ch);
        return;
    }
    if (ch < CHANNELS) count[ch]++;
}

static void enqueue(Queued *q, int *n, ENetPacket *p, enet_uint8 ch) {
    if (*n == QUEUE_MAX) {
        enet_packet_destroy(p);
        return;
    }
    q[*n].packet = p;
    q[*n].channel = ch;
    (*n)++;
}

static void drain(void) {
    if (!host_up || !join_up) return;
    for (int i = 0; i < to_host_n; i++) {
        pass(host_peer, to_host[i].packet, to_host[i].channel, passed_to_host);
        enet_packet_destroy(to_host[i].packet);
    }
    for (int i = 0; i < to_join_n; i++) {
        pass(join_peer, to_join[i].packet, to_join[i].channel, passed_to_join);
        enet_packet_destroy(to_join[i].packet);
    }
    if (to_host_n || to_join_n)
        fprintf(stderr, "relay: passed on %d queued to the host, %d to the joiner\n",
                to_host_n, to_join_n);
    to_host_n = to_join_n = 0;
}

static void connect_host(ENetHost *client, ENetAddress *addr) {
    host_peer = enet_host_connect(client, addr, CHANNELS, 0);
    if (!host_peer) {
        fprintf(stderr, "relay: enet_host_connect failed\n");
        exit(1);
    }
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: relay HOST_PORT JOIN_PORT SECONDS\n");
        return 2;
    }
    int host_port = atoi(argv[1]), join_port = atoi(argv[2]);
    time_t until = time(NULL) + atoi(argv[3]);
    if (enet_initialize() != 0) return 1;
    fprintf(stderr, "relay: ENet %u.%u.%u\n", ENET_VERSION_MAJOR, ENET_VERSION_MINOR,
            ENET_VERSION_PATCH);

    ENetAddress listen = {.host = ENET_HOST_ANY, .port = (enet_uint16)join_port};
    ENetHost *server = enet_host_create(&listen, 1, CHANNELS, 0, 0);
    ENetHost *client = enet_host_create(NULL, 1, CHANNELS, 0, 0);
    if (!server || !client) {
        fprintf(stderr, "relay: enet_host_create failed\n");
        return 1;
    }
    ENetAddress host_addr = {.port = (enet_uint16)host_port};
    enet_address_set_host_ip(&host_addr, "127.0.0.1");
    connect_host(client, &host_addr);

    ENetEvent ev;
    while (time(NULL) < until) {
        while (enet_host_service(client, &ev, 1) > 0) {
            switch (ev.type) {
            case ENET_EVENT_TYPE_CONNECT:
                host_up = 1;
                fprintf(stderr, "relay: reached the host\n");
                drain();
                break;
            case ENET_EVENT_TYPE_DISCONNECT:
                if (host_up) fprintf(stderr, "relay: the host went away\n");
                host_up = 0;
                connect_host(client, &host_addr);
                break;
            case ENET_EVENT_TYPE_RECEIVE:
                if (host_up && join_up) {
                    pass(join_peer, ev.packet, ev.channelID, passed_to_join);
                    enet_packet_destroy(ev.packet);
                } else {
                    enqueue(to_join, &to_join_n, ev.packet, ev.channelID);
                }
                break;
            default:
                break;
            }
        }
        while (enet_host_service(server, &ev, 1) > 0) {
            switch (ev.type) {
            case ENET_EVENT_TYPE_CONNECT:
                join_peer = ev.peer;
                join_up = 1;
                fprintf(stderr, "relay: the joiner connected\n");
                drain();
                break;
            case ENET_EVENT_TYPE_DISCONNECT:
                fprintf(stderr, "relay: the joiner went away\n");
                join_up = 0;
                join_peer = NULL;
                break;
            case ENET_EVENT_TYPE_RECEIVE:
                if (host_up && join_up) {
                    pass(host_peer, ev.packet, ev.channelID, passed_to_host);
                    enet_packet_destroy(ev.packet);
                } else {
                    enqueue(to_host, &to_host_n, ev.packet, ev.channelID);
                }
                break;
            default:
                break;
            }
        }
        enet_host_flush(client);
        enet_host_flush(server);
    }
    printf("to the host (handshake, game state, player state, profiles): %ld %ld %ld %ld\n",
           passed_to_host[0], passed_to_host[1], passed_to_host[2], passed_to_host[3]);
    printf("to the joiner (handshake, game state, player state, profiles): %ld %ld %ld %ld\n",
           passed_to_join[0], passed_to_join[1], passed_to_join[2], passed_to_join[3]);
    enet_host_destroy(server);
    enet_host_destroy(client);
    enet_deinitialize();
    return 0;
}
