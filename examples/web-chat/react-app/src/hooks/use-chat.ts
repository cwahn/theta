import { useCallback, useEffect, useState } from "react";
import {
  initTheta,
  spawnChatManager,
  bindChatManager,
  lookupChatManager,
  useChatRoomView,
  useChatManagerView,
  type ChatRoomRef,
  type ChatManagerRef,
  type ChatMessage,
  type RoomInfo,
} from "theta:actors";

type Status = "loading" | "ready" | "creating" | "joining" | "connected" | "error";

export type { ChatMessage, RoomInfo };

export function useChat() {
  const [status, setStatus] = useState<Status>("loading");
  const [error, setError] = useState<string | null>(null);
  const [myKey, setMyKey] = useState("");
  const [role, setRole] = useState<"host" | "guest" | null>(null);

  const [managerRef, setManagerRef] = useState<ChatManagerRef | null>(null);
  const [roomRef, setRoomRef] = useState<ChatRoomRef | null>(null);

  // View is the primary reactive data channel.
  const rooms = useChatManagerView(managerRef) ?? [];
  const messages = useChatRoomView(roomRef) ?? [];

  // Auto-select the first room when rooms arrive and none is selected.
  useEffect(() => {
    if (!roomRef && rooms.length > 0) {
      setRoomRef(rooms[0].room);
    }
  }, [roomRef, rooms]);

  useEffect(() => {
    let cancelled = false;
    initTheta()
      .then((key) => {
        if (!cancelled) {
          setMyKey(key);
          setStatus("ready");
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setError(String(e));
          setStatus("error");
        }
      });
    return () => { cancelled = true; };
  }, []);

  // Host: spawn ChatManager + bind for remote discovery.
  const createHost = useCallback(async () => {
    setStatus("creating");
    try {
      const mgr = spawnChatManager({ rooms: {} });
      bindChatManager("manager", mgr);
      setManagerRef(mgr);
      // ask returns confirmation + ref for auto-select.
      const info: RoomInfo = await mgr.ask({ CreateRoom: { name: "general" } });
      setRoomRef(info.room);
      setRole("host");
      setStatus("connected");
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }, []);

  // Guest: remote lookup → manager View populates rooms reactively.
  const joinHost = useCallback(async (hostKey: string) => {
    setStatus("joining");
    try {
      const mgr = await lookupChatManager(`iroh://manager@${hostKey}`);
      setManagerRef(mgr);
      setRole("guest");
      setStatus("connected");
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }, []);

  // Create additional room (host only). ask returns confirmation.
  const createRoom = useCallback(async (name: string) => {
    if (!managerRef) return;
    try {
      const info: RoomInfo = await managerRef.ask({ CreateRoom: { name } });
      setRoomRef(info.room);
    } catch (e) {
      setError(String(e));
    }
  }, [managerRef]);

  const selectRoom = useCallback((info: RoomInfo) => {
    setRoomRef(info.room);
  }, []);

  const sendMessage = useCallback((author: string, text: string) => {
    roomRef?.tell({ SendMessage: { author, text } });
  }, [roomRef]);

  return {
    status, error, myKey, role, messages, rooms,
    createHost, joinHost, createRoom, selectRoom, sendMessage,
    currentRoom: roomRef, peerKey: myKey,
  };
}
