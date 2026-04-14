import { useCallback, useEffect, useRef, useState } from "react";
import {
  initTheta,
  spawnChatManager,
  bindChatRoom,
  lookupChatRoom,
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
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [rooms, setRooms] = useState<RoomInfo[]>([]);
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

  const roomStreamRef = useRef(false);

  const startRoomStream = useCallback(async (chatRoom: ChatRoomRef) => {
    if (roomStreamRef.current) return;
    roomStreamRef.current = true;
    await chatRoom.initStream((state: ChatMessage[]) => {
      setMessages([...state]);
    });
  }, []);

  const selectRoom = useCallback(async (info: RoomInfo) => {
    roomStreamRef.current = false;
    roomRef.current = info.room;
    setMessages([]);
    await startRoomStream(info.room);
  }, [startRoomStream]);

  const createRoom = useCallback(async (roomName?: string) => {
    if (!managerRef.current) {
      setStatus("creating");
      try {
        const manager = spawnChatManager({ rooms: {} });
        managerRef.current = manager;

        // Stream manager view to get live room list (Vec<RoomInfo>)
        await manager.initStream((roomList: RoomInfo[]) => {
          setRooms([...roomList]);
        });

        setRole("host");
        setPeerKey(myKey);
        setStatus("connected");
      } catch (e) {
        setError(String(e));
        setStatus("error");
        return;
      }
    }

    try {
      const name = roomName || "general";
      const info: RoomInfo = await managerRef.current.ask({ CreateRoom: { name } });
      console.log("[ChatManager] Created room:", info.name, info.room.id);

      // Auto-select the first room or newly created room
      if (!roomRef.current) {
        bindChatRoom("chat", info.room);
        await selectRoom(info);
      }
    } catch (e) {
      setError(String(e));
    }
  }, [myKey, selectRoom]);

  const joinRoom = useCallback(async (hostKey: string) => {
    setStatus("joining");
    try {
      const url = `iroh://chat@${hostKey}`;
      const chatRoom = await lookupChatRoom(url);
      roomRef.current = chatRoom;
      setRole("guest");
      setPeerKey(hostKey);
      setStatus("connected");
      await startRoomStream(chatRoom);
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }, [startRoomStream]);

  const sendMessage = useCallback((author: string, text: string) => {
    roomRef.current?.tell({ SendMessage: { author, text } });
  }, []);

  return {
    status, error, myKey, messages, rooms, role, peerKey,
    createRoom, joinRoom, sendMessage, selectRoom,
    currentRoom: roomRef.current,
  };
}
