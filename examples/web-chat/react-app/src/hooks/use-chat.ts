import { useCallback, useEffect, useRef, useState } from "react";
import {
  initTheta,
  spawnChatManager,
  bindChatRoom,
  lookupChatRoom,
  type ChatRoomRef,
  type ChatManagerRef,
  type ChatMessage,
} from "theta:actors";

type Status = "loading" | "ready" | "creating" | "joining" | "connected" | "error";

export type { ChatMessage };

export function useChat() {
  const [status, setStatus] = useState<Status>("loading");
  const [error, setError] = useState<string | null>(null);
  const [myKey, setMyKey] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [role, setRole] = useState<"host" | "guest" | null>(null);
  const [peerKey, setPeerKey] = useState("");
  const roomRef = useRef<ChatRoomRef | null>(null);
  const managerRef = useRef<ChatManagerRef | null>(null);

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

  const streamActiveRef = useRef(false);

  const startStream = useCallback(async (chatRoom: ChatRoomRef) => {
    if (streamActiveRef.current) return;
    streamActiveRef.current = true;
    await chatRoom.initStream((state: ChatMessage[]) => {
      setMessages([...state]);
    });
  }, []);

  const createRoom = useCallback(async () => {
    setStatus("creating");
    try {
      const manager = spawnChatManager({});
      managerRef.current = manager;

      const roomId = await manager.ask({ CreateRoom: { name: "chat" } });
      console.log("[ChatManager] Created room:", roomId);

      const chatRoom = await manager.ask({ ResolveRoom: { room_id: roomId } });
      console.log("[ChatManager] Resolved room:", chatRoom.id);

      bindChatRoom("chat", chatRoom);
      roomRef.current = chatRoom;
      setRole("host");
      setPeerKey(myKey);
      setStatus("connected");
      await startStream(chatRoom);
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }, [myKey, startStream]);

  const joinRoom = useCallback(async (hostKey: string) => {
    setStatus("joining");
    try {
      const url = `iroh://chat@${hostKey}`;
      const chatRoom = await lookupChatRoom(url);
      roomRef.current = chatRoom;
      setRole("guest");
      setPeerKey(hostKey);
      setStatus("connected");
      await startStream(chatRoom);
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }, [startStream]);

  const sendMessage = useCallback((author: string, text: string) => {
    roomRef.current?.tell({ SendMessage: { author, text } });
  }, []);

  return { status, error, myKey, messages, role, peerKey, createRoom, joinRoom, sendMessage };
}
